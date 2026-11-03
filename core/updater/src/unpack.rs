//! Unpacking a verified update payload into `versions/<v>/` (B8 §5, A3).
//!
//! The archive has already passed its hash check, so this is defence in
//! depth against a bad build or a compromised build machine: nothing in the
//! archive may write outside the new folder, overwrite anything, hide in an
//! NTFS stream, or collide with another entry on a case-insensitive disk.
//!
//! Rules for every entry name:
//! - UTF-8, `/`-separated, relative; no empty, `.` or `..` component, no
//!   `\`, `:`, NUL or other control character;
//! - no component ending in `.` or space, no Windows device name
//!   (`CON`, `nul.txt`, `COM1`, ...);
//! - each component at most [`MAX_COMPONENT_BYTES`], the whole name at most
//!   [`MAX_PATH_BYTES`];
//! - no two entries equal after lowercasing (file/file or file/folder);
//! - regular files and folders only: symlinks are refused (the Windows
//!   payload has none; Linux, which needs them, is deferred).
//!
//! Files are created with `create_new` (never through an existing path),
//! sizes are counted as written, not as declared, and the folder is renamed
//! from `<v>.partial` to `<v>` only after everything is written and synced.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use crate::{Error, VerifiedArtifact};

/// Longest single path component, in bytes (NTFS and ext4 allow 255).
pub const MAX_COMPONENT_BYTES: usize = 255;
/// Longest entry name, in bytes. Keeps `<root>\versions\<v>\<entry>` well
/// inside what Windows tools handle.
pub const MAX_PATH_BYTES: usize = 1024;
/// Most entries in one payload (an Electron app has a few hundred).
pub const MAX_ENTRIES: usize = 20_000;
/// Most bytes unpacked, whatever the archive size.
pub const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most bytes unpacked per byte of archive (zip-bomb guard).
pub const MAX_EXPANSION: u64 = 4;

fn bad(why: impl Into<String>) -> Error {
    Error::Unpack(why.into())
}

const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1",
    "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Checks one entry name; returns its components.
pub fn check_entry_name(name: &str) -> Result<Vec<&str>, Error> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty() || name.len() > MAX_PATH_BYTES {
        return Err(bad(format!("entry name length {}", name.len())));
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        let shown = || format!("bad entry name {name:?}");
        if part.is_empty() || part == "." || part == ".." || part.len() > MAX_COMPONENT_BYTES {
            return Err(bad(shown()));
        }
        if part.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
            return Err(bad(shown()));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(bad(shown()));
        }
        let stem = part.split('.').next().unwrap_or(part).trim_end().to_ascii_lowercase();
        if DEVICE_NAMES.contains(&stem.as_str()) {
            return Err(bad(shown()));
        }
        parts.push(part);
    }
    Ok(parts)
}

/// Unpacks `archive`, the `.zip` that `artifact` proves was verified, into
/// `versions_dir/<version>`, via `<version>.partial`. `required` (e.g.
/// `mvs.exe`) must be a regular file in the archive. Returns the new folder.
///
/// Size budget: the manifest's signed `unpacked_bytes`, which must then
/// match exactly; without it, [`MAX_EXPANSION`] × the archive size. Never
/// more than [`MAX_UNPACKED_BYTES`].
///
/// Fails if `versions_dir/<version>` already exists: the caller decides
/// whether a leftover folder may be removed.
pub fn unpack_zip<R: Read + Seek>(
    archive: R,
    artifact: &VerifiedArtifact,
    versions_dir: &Path,
    required: &str,
) -> Result<PathBuf, Error> {
    let version = artifact.version();
    let final_dir = versions_dir.join(version.to_string());
    if final_dir.exists() {
        return Err(bad(format!("{} already exists", final_dir.display())));
    }
    let partial = versions_dir.join(format!("{version}.partial"));
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    fs::create_dir_all(versions_dir)?;
    fs::create_dir(&partial)?;
    let budget = artifact
        .unpacked_bytes()
        .unwrap_or_else(|| artifact.size().saturating_mul(MAX_EXPANSION))
        .min(MAX_UNPACKED_BYTES);
    match extract(archive, budget, artifact.unpacked_bytes(), &partial, required) {
        Ok(()) => {
            fs::rename(&partial, &final_dir)?;
            Ok(final_dir)
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

fn extract<R: Read + Seek>(
    archive: R,
    budget: u64,
    exact: Option<u64>,
    dest: &Path,
    required: &str,
) -> Result<(), Error> {
    let mut zip = zip::ZipArchive::new(archive).map_err(|e| bad(format!("not a zip archive: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(bad(format!("{} entries, more than {MAX_ENTRIES}", zip.len())));
    }
    let mut written: u64 = 0;
    // Lowercased path -> is a folder. Every file's parent folders are
    // recorded too, so a file and a folder cannot share a name either.
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut files: HashSet<String> = HashSet::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| bad(format!("entry {i}: {e}")))?;
        let name = String::from_utf8(entry.name_raw().to_vec())
            .map_err(|_| bad(format!("entry {i}: name is not UTF-8")))?;
        let parts = check_entry_name(&name)?;
        if entry.is_symlink() {
            return Err(bad(format!("{name:?} is a symlink")));
        }
        let is_dir = entry.is_dir();
        if !is_dir && !entry.is_file() {
            return Err(bad(format!("{name:?} is not a regular file")));
        }
        for depth in 1..=parts.len() {
            let key = parts[..depth].join("/").to_lowercase();
            let here_is_dir = depth < parts.len() || is_dir;
            match seen.get(&key) {
                Some(&prev_dir) if !(prev_dir && here_is_dir) => {
                    return Err(bad(format!("{name:?} collides with another entry")));
                }
                Some(_) => {}
                None => {
                    seen.insert(key, here_is_dir);
                }
            }
        }
        let path = parts.iter().fold(dest.to_path_buf(), |p, c| p.join(c));
        if is_dir {
            fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = create_new(&path, entry.unix_mode())?;
        let left = budget - written;
        let n = io::copy(&mut (&mut entry).take(left + 1), &mut out)?;
        if n > left {
            return Err(bad(format!("unpacked size over {budget} bytes")));
        }
        written += n;
        out.sync_all()?;
        files.insert(parts.join("/"));
    }
    if !files.contains(required) {
        return Err(bad(format!("payload has no {required}")));
    }
    if let Some(expected) = exact.filter(|&e| e != written) {
        return Err(bad(format!("unpacked {written} bytes, the manifest says {expected}")));
    }
    sync_dir(dest);
    Ok(())
}

/// A new file, never through or over an existing path (A3).
fn create_new(path: &Path, mode: Option<u32>) -> Result<File, Error> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let exec = mode.is_some_and(|m| m & 0o111 != 0);
        opts.mode(if exec { 0o755 } else { 0o644 }).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(opts.open(path)?)
}

/// Makes the new folder's entries durable before the rename (best effort:
/// Windows cannot open folders this way).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use crate::Version;
    use zip::write::SimpleFileOptions;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mvs-unpack-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A zip with the given (name, contents); a name ending in `/` is a folder.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    fn unpack(tmp: &Tmp, bytes: &[u8]) -> Result<PathBuf, Error> {
        unpack_signed(tmp, bytes, None)
    }

    fn unpack_signed(tmp: &Tmp, bytes: &[u8], unpacked: Option<u64>) -> Result<PathBuf, Error> {
        let artifact = VerifiedArtifact::for_tests_sized(v("1.2.0"), bytes.len() as u64, unpacked);
        unpack_zip(Cursor::new(bytes), &artifact, &tmp.0.join("versions"), "MVS.exe")
    }

    #[test]
    fn signed_unpacked_size_replaces_the_ratio_and_must_match() {
        let tmp = Tmp::new("signed");
        // Compresses far beyond MAX_EXPANSION: refused by the ratio alone...
        let zeros = vec![0u8; 1024 * 1024];
        let bytes = zip_of(&[("MVS.exe", b"exe"), ("app.asar", &zeros)]);
        let total = 3 + zeros.len() as u64;
        assert!(unpack(&tmp, &bytes).is_err());
        // ...accepted with the signed size,
        unpack_signed(&tmp, &bytes, Some(total)).unwrap();
        fs::remove_dir_all(tmp.0.join("versions")).unwrap();
        // ...and refused when the signed size is smaller or larger than the truth.
        for wrong in [total - 1, total + 1] {
            let err = unpack_signed(&tmp, &bytes, Some(wrong)).unwrap_err();
            assert!(matches!(err, Error::Unpack(_)), "{wrong}: {err:?}");
            assert!(!tmp.0.join("versions").join("1.2.0").exists());
        }
    }

    #[test]
    fn good_payload_unpacks_into_the_version_folder() {
        let tmp = Tmp::new("good");
        let bytes = zip_of(&[
            ("MVS.exe", b"exe"),
            ("resources/", b""),
            ("resources/app.asar", b"asar"),
            ("locales/he.pak", b"he"),
        ]);
        let dir = unpack(&tmp, &bytes).unwrap();
        assert_eq!(dir, tmp.0.join("versions").join("1.2.0"));
        assert_eq!(fs::read(dir.join("resources").join("app.asar")).unwrap(), b"asar");
        assert!(!tmp.0.join("versions").join("1.2.0.partial").exists());
    }

    #[test]
    fn hostile_names_are_refused() {
        for name in [
            "../evil",
            "a/../../evil",
            "/abs",
            "a//b",
            "./x",
            "a\\b",
            "C:x",
            "file.exe:stream",
            "trailing.",
            "trailing ",
            "CON",
            "nul.txt",
            "Com1.dll",
            "lpt9",
            "a\u{0}b",
            "bad\u{7}",
            "q?",
            "",
        ] {
            assert!(matches!(check_entry_name(name), Err(Error::Unpack(_))), "{name:?} accepted");
        }
        assert!(check_entry_name(&"a".repeat(MAX_COMPONENT_BYTES + 1)).is_err());
        assert!(check_entry_name(&format!("{}/x", "a/".repeat(MAX_PATH_BYTES / 2))).is_err());
        for ok in ["MVS.exe", "resources/app.asar", "locales/he.pak", "console.txt", "a.b.c", "dir/"] {
            assert!(check_entry_name(ok).is_ok(), "{ok:?} refused");
        }
    }

    #[test]
    fn hostile_entry_fails_and_leaves_nothing_behind() {
        let tmp = Tmp::new("hostile");
        let bytes = zip_of(&[("MVS.exe", b"exe"), ("../escape.txt", b"x")]);
        assert!(matches!(unpack(&tmp, &bytes), Err(Error::Unpack(_))));
        assert!(!tmp.0.join("escape.txt").exists());
        assert!(!tmp.0.join("versions").join("1.2.0").exists());
        assert!(!tmp.0.join("versions").join("1.2.0.partial").exists());
    }

    #[test]
    fn case_collisions_are_refused() {
        let tmp = Tmp::new("case");
        for entries in [
            vec![("MVS.exe", &b"a"[..]), ("mvs.EXE", &b"b"[..])],
            vec![("MVS.exe", &b"a"[..]), ("Res/x", &b"b"[..]), ("res", &b"c"[..])],
            vec![("MVS.exe", &b"a"[..]), ("res", &b"c"[..]), ("RES/x", &b"b"[..])],
        ] {
            assert!(matches!(unpack(&tmp, &zip_of(&entries)), Err(Error::Unpack(_))), "{entries:?}");
        }
        // The same folder named twice, in any case, is fine.
        assert!(unpack(&tmp, &zip_of(&[("MVS.exe", b"a"), ("res/", b""), ("RES/x", b"b")])).is_ok());
    }

    #[test]
    fn zip_bomb_is_stopped_by_the_size_budget() {
        let tmp = Tmp::new("bomb");
        let zeros = vec![0u8; 4 * 1024 * 1024];
        let bytes = zip_of(&[("MVS.exe", b"exe"), ("big.bin", &zeros)]);
        assert!(bytes.len() * 4 < zeros.len(), "test archive should compress well");
        let err = unpack(&tmp, &bytes).unwrap_err();
        assert!(matches!(&err, Error::Unpack(why) if why.contains("unpacked size")), "{err:?}");
    }

    #[test]
    fn missing_required_entry_and_existing_folder_are_refused() {
        let tmp = Tmp::new("required");
        assert!(matches!(unpack(&tmp, &zip_of(&[("other.exe", b"x")])), Err(Error::Unpack(_))));
        let ok = zip_of(&[("MVS.exe", b"x")]);
        unpack(&tmp, &ok).unwrap();
        assert!(matches!(unpack(&tmp, &ok), Err(Error::Unpack(why)) if why.contains("already exists")));
    }

    #[test]
    fn a_stale_partial_folder_is_replaced() {
        let tmp = Tmp::new("partial");
        let partial = tmp.0.join("versions").join("1.2.0.partial");
        fs::create_dir_all(partial.join("junk")).unwrap();
        let dir = unpack(&tmp, &zip_of(&[("MVS.exe", b"x")])).unwrap();
        assert!(!dir.join("junk").exists());
    }

    #[test]
    fn symlinks_are_refused() {
        let tmp = Tmp::new("symlink");
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        w.start_file("MVS.exe", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.add_symlink("link", "/etc/passwd", opts).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        assert!(matches!(unpack(&tmp, &bytes), Err(Error::Unpack(why)) if why.contains("symlink")));
    }

    #[test]
    fn not_a_zip_is_refused() {
        let tmp = Tmp::new("notzip");
        assert!(matches!(unpack(&tmp, b"definitely not a zip"), Err(Error::Unpack(_))));
    }
}
