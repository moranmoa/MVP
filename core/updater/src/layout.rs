//! Where the in-app update keeps things (B8 §0, §2), and how state files are
//! written and locked.
//!
//! ```text
//! <install>/                 $INSTDIR (per-user NSIS): the base copy
//!   mvs[.exe]                base version, put there by the installer
//!   mvs-launcher[.exe]       stable launcher, only the installer replaces it
//!   versions/<v>/mvs[.exe]   unpacked updates
//! <state>/                   %LOCALAPPDATA%\MVS\update (outside $INSTDIR:
//!   state.json, state.json.bak, policy.json, root.json,
//!   db-snapshot.json, lock, downloads/      an installer upgrade wipes $INSTDIR)
//! ```
//!
//! Every path is built here from fixed names and parsed [`Version`]s, never
//! from text read out of a state file (A1).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{Error, Version};

/// The app executable (electron-builder `executableName`).
pub const APP_EXE: &str = if cfg!(windows) { "mvs.exe" } else { "mvs" };
/// The launcher executable.
pub const LAUNCHER_EXE: &str = if cfg!(windows) { "mvs-launcher.exe" } else { "mvs-launcher" };
/// State files are small; anything bigger is refused before parsing.
pub const MAX_STATE_FILE_BYTES: u64 = 64 * 1024;

/// The install folder and the per-user state folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateDirs {
    install: PathBuf,
    state: PathBuf,
}

impl UpdateDirs {
    pub fn new(install: impl Into<PathBuf>, state: impl Into<PathBuf>) -> Self {
        UpdateDirs { install: install.into(), state: state.into() }
    }

    /// The state folder for this user: `%LOCALAPPDATA%\MVS\update` on
    /// Windows, `$XDG_DATA_HOME/MVS/update` (or `~/.local/share/...`)
    /// elsewhere. `None` if the environment does not say.
    pub fn default_state_dir() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        };
        base.filter(|b| b.is_absolute()).map(|b| b.join("MVS").join("update"))
    }

    pub fn install(&self) -> &Path {
        &self.install
    }
    pub fn state(&self) -> &Path {
        &self.state
    }
    pub fn launcher(&self) -> PathBuf {
        self.install.join(LAUNCHER_EXE)
    }
    /// The installer's copy.
    pub fn base_exe(&self) -> PathBuf {
        self.install.join(APP_EXE)
    }
    pub fn versions(&self) -> PathBuf {
        self.install.join("versions")
    }
    /// Built from a parsed version only (A1).
    pub fn version_dir(&self, v: Version) -> PathBuf {
        self.versions().join(v.to_string())
    }
    pub fn version_exe(&self, v: Version) -> PathBuf {
        self.version_dir(v).join(APP_EXE)
    }
    pub fn state_file(&self) -> PathBuf {
        self.state.join("state.json")
    }
    /// The last good `state.json`, kept before every change (A6).
    pub fn state_backup(&self) -> PathBuf {
        self.state.join("state.json.bak")
    }
    pub fn policy_file(&self) -> PathBuf {
        self.state.join("policy.json")
    }
    pub fn root_file(&self) -> PathBuf {
        self.state.join("root.json")
    }
    pub fn snapshot_file(&self) -> PathBuf {
        self.state.join("db-snapshot.json")
    }
    pub fn lock_file(&self) -> PathBuf {
        self.state.join("lock")
    }
    /// Written by the launcher on a rollback, read once by the core.
    pub fn rollback_notice(&self) -> PathBuf {
        self.state.join("rollback.json")
    }
    /// The staged version's signed notes, kept for `update.installed`.
    pub fn staged_notes(&self) -> PathBuf {
        self.state.join("staged-notes.json")
    }
    pub fn downloads(&self) -> PathBuf {
        self.state.join("downloads")
    }
}

/// Reads a state file: `Ok(None)` if it does not exist, an error if it is
/// larger than [`MAX_STATE_FILE_BYTES`] or unreadable.
pub fn read_state_file(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut buf = Vec::new();
    f.take(MAX_STATE_FILE_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_STATE_FILE_BYTES {
        return Err(Error::InvalidState(format!("{} is larger than {MAX_STATE_FILE_BYTES} bytes", path.display())));
    }
    Ok(Some(buf))
}

/// Replaces `path` with `bytes` atomically: a synced temporary file in the
/// same folder, renamed over it. A crash leaves the old or the new file,
/// never half of one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let dir = path.parent().ok_or_else(|| Error::Io(format!("{} has no folder", path.display())))?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Writes a new `state.json`, first copying the current one (if it is
/// readable and `still_good` accepts it) to `state.json.bak` (A6).
pub fn save_state(dirs: &UpdateDirs, bytes: &[u8], still_good: impl Fn(&[u8]) -> bool) -> Result<(), Error> {
    if let Ok(Some(current)) = read_state_file(&dirs.state_file()) {
        if still_good(&current) {
            write_atomic(&dirs.state_backup(), &current)?;
        }
    }
    write_atomic(&dirs.state_file(), bytes)
}

/// The one-writer lock on the state folder, held while reading and writing
/// state (launcher and core). Released on drop or when the process exits,
/// so a crash cannot leave it stuck.
#[derive(Debug)]
pub struct UpdateLock {
    _file: File,
}

impl UpdateLock {
    /// Waits up to `timeout` for the lock.
    pub fn acquire(dirs: &UpdateDirs, timeout: Duration) -> Result<UpdateLock, Error> {
        fs::create_dir_all(dirs.state())?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dirs.lock_file())?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(UpdateLock { _file: file }),
                Err(fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(Error::Io("update state is locked by another process".into()))
                }
                Err(fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mvs-layout-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn dirs(&self) -> UpdateDirs {
            UpdateDirs::new(self.0.join("install"), self.0.join("state"))
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn paths_are_built_from_fixed_names_and_versions() {
        let d = UpdateDirs::new("/i", "/s");
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!(d.version_exe(v), Path::new("/i").join("versions").join("1.2.3").join(APP_EXE));
        assert_eq!(d.base_exe(), Path::new("/i").join(APP_EXE));
        assert_eq!(d.launcher(), Path::new("/i").join(LAUNCHER_EXE));
        assert_eq!(d.state_backup(), Path::new("/s").join("state.json.bak"));
        // State lives outside the install folder (an installer upgrade wipes it).
        assert!(!d.state_file().starts_with(d.install()));
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let t = Tmp::new("atomic");
        let p = t.0.join("state").join("x.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn save_keeps_the_last_good_state_as_backup() {
        let t = Tmp::new("backup");
        let d = t.dirs();
        let good = |b: &[u8]| b.starts_with(b"good");
        save_state(&d, b"good-1", good).unwrap();
        assert_eq!(read_state_file(&d.state_backup()).unwrap(), None);
        save_state(&d, b"good-2", good).unwrap();
        assert_eq!(read_state_file(&d.state_backup()).unwrap().unwrap(), b"good-1");
        // A corrupt current file never becomes the backup.
        fs::write(d.state_file(), b"garbage").unwrap();
        save_state(&d, b"good-3", good).unwrap();
        assert_eq!(read_state_file(&d.state_backup()).unwrap().unwrap(), b"good-1");
        assert_eq!(read_state_file(&d.state_file()).unwrap().unwrap(), b"good-3");
    }

    #[test]
    fn oversized_state_file_is_refused() {
        let t = Tmp::new("big");
        let p = t.0.join("big.json");
        fs::write(&p, vec![b' '; MAX_STATE_FILE_BYTES as usize + 1]).unwrap();
        assert!(matches!(read_state_file(&p), Err(Error::InvalidState(_))));
        assert_eq!(read_state_file(&t.0.join("missing.json")).unwrap(), None);
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let t = Tmp::new("lock");
        let d = t.dirs();
        let held = UpdateLock::acquire(&d, Duration::ZERO).unwrap();
        // A second handle (as another process would have) cannot take it.
        assert!(UpdateLock::acquire(&d, Duration::from_millis(120)).is_err());
        drop(held);
        assert!(UpdateLock::acquire(&d, Duration::ZERO).is_ok());
    }
}
