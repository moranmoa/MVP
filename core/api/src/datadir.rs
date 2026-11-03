//! The sidecar's data dir, owner-only. Same approach as `sync`'s key file
//! (`core/sync/src/keyfile.rs`), for a directory:
//!
//! - Unix: mode 0700, also on an existing dir.
//! - Windows: a protected DACL with one entry, full control for the current
//!   user's SID, inherited by everything created inside (so `core.db` gets
//!   it), checked with the system's `icacls`.

use std::fs;
use std::io;
use std::path::Path;

/// Creates `dir` if missing and makes it (and what is created in it from
/// now on) accessible to the current user only.
pub fn prepare(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    restrict(dir)
}

#[cfg(unix)]
fn restrict(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Replaces the dir's whole DACL, through a handle opened only to change
/// it, with a protected one: full control for the current user's SID,
/// inherited by files and subdirs. Then checks it with `icacls`.
#[cfg(windows)]
fn restrict(dir: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL, WRITE_DAC,
    };

    let fail = |what: String| io::Error::other(format!("could not restrict the data dir: {what}"));
    let system32 = PathBuf::from(
        std::env::var_os("SystemRoot").ok_or_else(|| fail("SystemRoot is not set".into()))?,
    )
    .join("System32");
    let run = |exe: &str, args: &[&std::ffi::OsStr]| -> io::Result<String> {
        let out = Command::new(system32.join(exe))
            .args(args)
            .output()
            .map_err(|e| fail(format!("{exe}: {e}")))?;
        if !out.status.success() {
            return Err(fail(format!("{exe} failed")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let who =
        run("whoami.exe", &["/user".as_ref(), "/fo".as_ref(), "csv".as_ref(), "/nh".as_ref()])?;
    let sid = who
        .trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"'))
        .filter(|s| {
            s.starts_with("S-1-") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .ok_or_else(|| fail("can't read the current user's SID".into()))?
        .to_owned();

    // A directory handle needs FILE_FLAG_BACKUP_SEMANTICS.
    let handle = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)?;
    // P = protected (no inheritance from above); OICI = inherited by files
    // and subdirs; FA = full access; one ACE for our SID.
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{sid})").encode_utf16().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` is freed with LocalFree below.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    } == 0
    {
        return Err(fail(format!("building the ACL: {}", io::Error::last_os_error())));
    }
    let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
    // SAFETY: `sd` is a valid descriptor from the call above; `dacl` points
    // into it; the handle is open with WRITE_DAC.
    let rc = unsafe {
        if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
        {
            Err(fail(format!("reading the ACL: {}", io::Error::last_os_error())))
        } else {
            match SetSecurityInfo(
                handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            ) {
                ERROR_SUCCESS => Ok(()),
                code => Err(fail(format!(
                    "setting the ACL: {}",
                    io::Error::from_raw_os_error(code as i32)
                ))),
            }
        }
    };
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd) };
    rc?;
    drop(handle);

    // Independent check: icacls lists one ACE per line after the path.
    let listing = run("icacls.exe", &[dir.as_os_str()])?;
    let aces = listing.lines().filter(|l| l.contains(":(")).count();
    if aces != 1 {
        return Err(fail(format!("expected one access entry, icacls shows {aces}")));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict(_dir: &Path) -> io::Result<()> {
    Err(io::Error::other("don't know how to restrict a directory on this platform"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn new_and_existing_dirs_end_up_0700() {
        let base = std::env::temp_dir().join(format!("mvs-datadir-{}", std::process::id()));
        let dir = base.join("data");
        super::prepare(&dir).unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::prepare(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
