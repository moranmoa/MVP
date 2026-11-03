//! Owner-only secret files (the node's libp2p key). Same approach as the
//! updater's root keys (`core/updater/src/main.rs`): the file is created
//! without overwriting, restricted to the current user before anything is
//! written, and removed again if that fails.
//!
//! - Unix: created with mode 0600; an existing key is tightened to 0600 on load.
//! - Windows: created with no sharing (so no handle can be opened before the
//!   ACL change), then given a protected DACL with one entry, full control
//!   for the current user's SID, and checked with the system's `icacls`.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Creates `path` holding `contents`, readable and writable by the current
/// user only. Fails if the file exists.
pub(crate) fn write_secret(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
        opts.share_mode(0).access_mode(GENERIC_WRITE | WRITE_DAC);
    }
    let mut file = opts.open(path)?;
    if let Err(err) = restrict_to_owner(&file, path) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io::Error::other(format!("could not restrict access to the key file: {err}; no key was written")));
    }
    file.write_all(contents)?;
    file.sync_all()
}

/// Makes an existing secret file owner-only where that is cheap to do
/// (Unix). Keys written by older versions may have been world-readable.
pub(crate) fn tighten(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(unix)]
fn restrict_to_owner(_file: &fs::File, _path: &Path) -> Result<(), String> {
    Ok(()) // created with mode 0600 above
}

/// Replaces the file's whole DACL, through our open handle, with a protected
/// one holding a single entry: full control for the current user's SID. The
/// result is then checked with the system's own `icacls`.
#[cfg(windows)]
fn restrict_to_owner(file: &fs::File, path: &Path) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR,
    };

    let system32 = PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?).join("System32");
    let run = |exe: &str, args: &[&std::ffi::OsStr]| -> Result<String, String> {
        let out = Command::new(system32.join(exe)).args(args).output().map_err(|e| format!("{exe}: {e}"))?;
        if !out.status.success() {
            return Err(format!("{exe} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let who = run("whoami.exe", &["/user".as_ref(), "/fo".as_ref(), "csv".as_ref(), "/nh".as_ref()])?;
    let sid = who
        .trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"'))
        .filter(|s| s.starts_with("S-1-") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .ok_or_else(|| format!("cannot read the current user's SID from {who:?}"))?
        .to_string();

    // P = protected (no inheritance); FA = full access; one ACE for our SID.
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;{sid})").encode_utf16().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` is freed with LocalFree below.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == 0 {
        return Err(format!("building the ACL failed: {}", io::Error::last_os_error()));
    }
    let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
    // SAFETY: `sd` is a valid descriptor from the call above; `dacl` points into it.
    let rc = unsafe {
        if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 || present == 0 {
            Err(format!("reading the ACL failed: {}", io::Error::last_os_error()))
        } else {
            match SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            ) {
                ERROR_SUCCESS => Ok(()),
                code => Err(format!("setting the ACL failed: {}", io::Error::from_raw_os_error(code as i32))),
            }
        }
    };
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd) };
    rc?;

    // Independent check: icacls lists one ACE per line after the path.
    let listing = run("icacls.exe", &[path.as_os_str()])?;
    let aces = listing.lines().filter(|l| l.contains(":(")).count();
    if aces != 1 {
        return Err(format!("expected exactly one access entry, icacls shows {aces}:\n{listing}"));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict_to_owner(_file: &fs::File, _path: &Path) -> Result<(), String> {
    Err("don't know how to restrict file access on this platform".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn secret_files_are_owner_only_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("sync-keyfile-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.key");
        let _ = fs::remove_file(&path);
        write_secret(&path, b"secret").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_secret(&path, b"other").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"secret");

        // An old, world-readable key is tightened on load.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        tighten(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
