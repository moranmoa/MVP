//! What `mvs-launcher` does (B8-3), as plain functions so it can be tested.
//!
//! The launcher is the one binary an update never replaces. It has no
//! network and verifies nothing: the core verified the payload before
//! staging it. At every start it:
//!
//! 1. takes [`UpdateLock`];
//! 2. loads `state.json`, else `state.json.bak`, else starts from the base
//!    version (A6). A state for a version whose folder is gone, or older than
//!    the base the installer put down, is re-based on the base;
//! 3. counts the boot ([`InstallState::begin_boot`]) with the revocations
//!    from `policy.json`;
//! 4. on a rollback, restores the database snapshot the new version took
//!    before migrating (§0), before anything opens the database;
//! 5. saves the state, and starts the chosen `mvs[.exe]`, built from the
//!    parsed version (A1), with argv forwarded verbatim (§0).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::layout::{read_state_file, save_state, write_atomic, UpdateDirs, UpdateLock};
use crate::{Boot, ClientPolicy, Error, InstallState, Version};

/// Trial boots a new version gets before the launcher rolls back (spec).
pub const LAUNCH_ATTEMPTS: u32 = 3;
/// The only thing the launcher adds to the app's command line.
pub const UPDATE_ROOT_ARG: &str = "--mvs-update-root=";
/// How long the launcher waits for the core to release the state lock.
pub const LOCK_WAIT: Duration = Duration::from_secs(10);

/// What to start, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub exe: PathBuf,
    pub boot: Boot,
    /// True when the database snapshot was put back (rollback).
    pub restored_snapshot: bool,
}

/// `db-snapshot.json`, written by the core before a schema migration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRecord {
    /// The version that migrated (and took the snapshot).
    version: Version,
    /// A plain file name in the core data folder: `core.db.pre-<schema>`.
    file: String,
}

fn is_snapshot_name(name: &str) -> bool {
    name.strip_prefix("core.db.pre-").is_some_and(|s| {
        (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

fn parses_as_state(bytes: &[u8]) -> bool {
    serde_json::from_slice::<InstallState>(bytes).is_ok()
}

/// Decides which version to start, updating the state on disk.
/// `base` is the version the installer put in the install folder;
/// `core_data` is the core's data folder (with `core.db`).
pub fn plan(dirs: &UpdateDirs, core_data: &Path, base: Version, platform: &str) -> Result<Plan, Error> {
    let _lock = UpdateLock::acquire(dirs, LOCK_WAIT)?;

    // A6: the state, else the last good copy, else a fresh start.
    let primary = read_state_file(&dirs.state_file()).ok().flatten().filter(|b| parses_as_state(b));
    let saved = primary.or_else(|| read_state_file(&dirs.state_backup()).ok().flatten().filter(|b| parses_as_state(b)));
    let (mut state, _) = InstallState::restore(saved.as_deref(), base, platform, LAUNCH_ATTEMPTS);

    // An installer upgrade wipes versions/: a missing folder means "the
    // base", not a failed start.
    if state.current() != base && !dirs.version_exe(state.current()).is_file() {
        state.rebase(base);
    }
    if state.trial().is_some_and(|t| !dirs.version_exe(t.version).is_file()) {
        state.forget_missing_trial();
    }

    let revoked: BTreeSet<Version> = read_state_file(&dirs.policy_file())
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice::<ClientPolicy>(&b).ok())
        .map(|p| p.revoked().clone())
        .unwrap_or_default();
    let boot = state.begin_boot(&revoked);

    let restored_snapshot = match boot {
        Boot::RolledBack { failed, .. } => restore_snapshot(dirs, core_data, failed)?,
        _ => false,
    };
    save_state(dirs, state.to_json().as_bytes(), parses_as_state)?;

    let version = match boot {
        Boot::Normal(v) | Boot::Trial(v) | Boot::RolledBack { boot: v, .. } => v,
        // The running version is revoked and nothing newer is staged: start
        // it anyway, so its updater can fetch the fix (downgrades never).
        Boot::RecoveryRequired => state.current(),
    };
    let exe = if version == base { dirs.base_exe() } else { dirs.version_exe(version) };
    // Last line of defence: never "start whatever is there".
    let exe = if exe.is_file() { exe } else { dirs.base_exe() };
    Ok(Plan { exe, boot, restored_snapshot })
}

/// Puts `core.db` back to the snapshot the failed version took before it
/// migrated. Only a snapshot recorded for exactly that version, with a
/// plain `core.db.pre-*` name, is used.
fn restore_snapshot(dirs: &UpdateDirs, core_data: &Path, failed: Version) -> Result<bool, Error> {
    let Some(bytes) = read_state_file(&dirs.snapshot_file())? else { return Ok(false) };
    let record = match serde_json::from_slice::<SnapshotRecord>(&bytes) {
        Ok(r) if r.version == failed && is_snapshot_name(&r.file) => r,
        _ => return Ok(false),
    };
    let snapshot = core_data.join(&record.file);
    if !snapshot.is_file() {
        return Ok(false);
    }
    // SQLite's journal files belong to the newer schema: drop them first.
    for leftover in ["core.db-wal", "core.db-shm", "core.db-journal"] {
        let _ = fs::remove_file(core_data.join(leftover));
    }
    fs::rename(&snapshot, core_data.join("core.db"))?;
    fs::remove_file(dirs.snapshot_file())?;
    Ok(true)
}

/// The command that starts the app: no shell, argv forwarded verbatim
/// (including `--` and everything after it), plus only the update root.
/// Node/Electron debugging switches from the environment are dropped.
pub fn command(plan: &Plan, dirs: &UpdateDirs, forwarded: &[OsString]) -> Command {
    let mut root = OsString::from(UPDATE_ROOT_ARG);
    root.push(dirs.install().as_os_str());
    let mut cmd = Command::new(&plan.exe);
    cmd.arg(root).args(forwarded);
    for var in ["NODE_OPTIONS", "ELECTRON_RUN_AS_NODE", "ELECTRON_ENABLE_LOGGING", "NODE_DEBUG"] {
        cmd.env_remove(var);
    }
    if let Some(dir) = plan.exe.parent() {
        cmd.current_dir(dir);
    }
    cmd
}

/// The core's data folder, where Electron's `userData` puts it
/// (`%APPDATA%\MVS\core`, `$XDG_CONFIG_HOME/MVS/core`).
pub fn default_core_data() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.filter(|b| b.is_absolute()).map(|b| b.join("MVS").join("core"))
}

/// Writes `db-snapshot.json` (for the core and for tests).
pub fn record_snapshot(dirs: &UpdateDirs, version: Version, file: &str) -> Result<(), Error> {
    if !is_snapshot_name(file) {
        return Err(Error::InvalidState(format!("bad snapshot name {file:?}")));
    }
    let json = serde_json::json!({ "version": version.to_string(), "file": file });
    write_atomic(&dirs.snapshot_file(), json.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VerifiedArtifact;
    use std::ffi::OsStr;

    const PLATFORM: &str = "windows-x86_64";

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    struct Env {
        root: PathBuf,
        dirs: UpdateDirs,
        data: PathBuf,
    }

    impl Env {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("mvs-launcher-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let dirs = UpdateDirs::new(root.join("install"), root.join("state"));
            let data = root.join("data");
            fs::create_dir_all(dirs.install()).unwrap();
            fs::create_dir_all(&data).unwrap();
            fs::write(dirs.base_exe(), b"base").unwrap();
            Env { root, dirs, data }
        }
        fn install_version(&self, s: &str) {
            fs::create_dir_all(self.dirs.version_dir(v(s))).unwrap();
            fs::write(self.dirs.version_exe(v(s)), s).unwrap();
        }
        fn stage(&self, s: &str) {
            let saved = read_state_file(&self.dirs.state_file()).unwrap();
            let (mut st, _) = InstallState::restore(saved.as_deref(), v("1.0.0"), PLATFORM, LAUNCH_ATTEMPTS);
            st.stage(&VerifiedArtifact::for_tests(v(s), PLATFORM), &BTreeSet::new()).unwrap();
            save_state(&self.dirs, st.to_json().as_bytes(), parses_as_state).unwrap();
        }
        fn healthy(&self) {
            let saved = read_state_file(&self.dirs.state_file()).unwrap();
            let (mut st, _) = InstallState::restore(saved.as_deref(), v("1.0.0"), PLATFORM, LAUNCH_ATTEMPTS);
            st.report_healthy().unwrap();
            save_state(&self.dirs, st.to_json().as_bytes(), parses_as_state).unwrap();
        }
        fn launch(&self) -> Plan {
            plan(&self.dirs, &self.data, v("1.0.0"), PLATFORM).unwrap()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn first_start_runs_the_base() {
        let e = Env::new("first");
        let p = e.launch();
        assert_eq!((p.exe, p.boot), (e.dirs.base_exe(), Boot::Normal(v("1.0.0"))));
    }

    #[test]
    fn staged_version_gets_three_starts_then_rolls_back() {
        let e = Env::new("rollback");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        for _ in 0..LAUNCH_ATTEMPTS {
            let p = e.launch();
            assert_eq!((p.exe, p.boot), (e.dirs.version_exe(v("1.1.0")), Boot::Trial(v("1.1.0"))));
        }
        let p = e.launch();
        assert_eq!(p.boot, Boot::RolledBack { failed: v("1.1.0"), boot: v("1.0.0") });
        assert_eq!(p.exe, e.dirs.base_exe());
        // And it stays rolled back.
        assert_eq!(e.launch().boot, Boot::Normal(v("1.0.0")));
    }

    #[test]
    fn healthy_trial_becomes_current() {
        let e = Env::new("healthy");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        e.launch();
        e.healthy();
        let p = e.launch();
        assert_eq!((p.exe, p.boot), (e.dirs.version_exe(v("1.1.0")), Boot::Normal(v("1.1.0"))));
    }

    #[test]
    fn rollback_restores_the_db_snapshot_before_the_old_version_starts() {
        let e = Env::new("snapshot");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        // 1.1.0 snapshots the 1.0.0 database, then migrates it.
        fs::write(e.data.join("core.db.pre-7"), b"schema 6").unwrap();
        fs::write(e.data.join("core.db"), b"schema 7").unwrap();
        fs::write(e.data.join("core.db-wal"), b"schema 7 wal").unwrap();
        record_snapshot(&e.dirs, v("1.1.0"), "core.db.pre-7").unwrap();
        for _ in 0..LAUNCH_ATTEMPTS {
            assert!(!e.launch().restored_snapshot);
            assert_eq!(fs::read(e.data.join("core.db")).unwrap(), b"schema 7");
        }
        let p = e.launch();
        assert!(p.restored_snapshot);
        assert_eq!(fs::read(e.data.join("core.db")).unwrap(), b"schema 6");
        assert!(!e.data.join("core.db-wal").exists());
        assert!(!e.dirs.snapshot_file().exists());
    }

    #[test]
    fn snapshot_for_another_version_or_with_a_path_is_ignored() {
        for (version, file) in [("1.2.0", "core.db.pre-7"), ("1.1.0", "../core.db.pre-7"), ("1.1.0", "core.db")] {
            let e = Env::new("badsnap");
            e.install_version("1.1.0");
            e.stage("1.1.0");
            fs::write(e.data.join("core.db"), b"schema 7").unwrap();
            fs::write(e.data.join("core.db.pre-7"), b"schema 6").unwrap();
            let json = serde_json::json!({ "version": version, "file": file }).to_string();
            fs::create_dir_all(e.dirs.state()).unwrap();
            fs::write(e.dirs.snapshot_file(), json).unwrap();
            for _ in 0..=LAUNCH_ATTEMPTS {
                assert!(!e.launch().restored_snapshot);
            }
            assert_eq!(fs::read(e.data.join("core.db")).unwrap(), b"schema 7", "{version} {file}");
        }
        assert!(record_snapshot(&Env::new("rec").dirs, v("1.1.0"), "../x").is_err());
    }

    #[test]
    fn missing_version_folder_starts_the_base_without_counting_a_failure() {
        let e = Env::new("wiped");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        e.launch();
        e.healthy();
        // An installer upgrade wiped versions/.
        fs::remove_dir_all(e.dirs.versions()).unwrap();
        let p = e.launch();
        assert_eq!((p.exe, p.boot), (e.dirs.base_exe(), Boot::Normal(v("1.0.0"))));
        // A staged version whose folder vanished is forgotten, not failed.
        e.install_version("1.2.0");
        e.stage("1.2.0");
        fs::remove_dir_all(e.dirs.version_dir(v("1.2.0"))).unwrap();
        assert_eq!(e.launch().boot, Boot::Normal(v("1.0.0")));
        e.install_version("1.2.0");
        e.stage("1.2.0");
        assert_eq!(e.launch().boot, Boot::Trial(v("1.2.0")));
    }

    #[test]
    fn corrupt_state_uses_the_backup_then_the_base() {
        let e = Env::new("corrupt");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        e.launch();
        e.healthy();
        e.launch(); // saves again, so the backup holds "current = 1.1.0"
        fs::write(e.dirs.state_file(), b"{ garbage").unwrap();
        assert_eq!(e.launch().boot, Boot::Normal(v("1.1.0")));
        fs::write(e.dirs.state_file(), b"{ garbage").unwrap();
        fs::write(e.dirs.state_backup(), b"also garbage").unwrap();
        let p = e.launch();
        assert_eq!((p.exe, p.boot), (e.dirs.base_exe(), Boot::Normal(v("1.0.0"))));
    }

    #[test]
    fn revoked_trial_is_never_started() {
        let e = Env::new("revoked");
        e.install_version("1.1.0");
        e.stage("1.1.0");
        let mut policy = serde_json::to_value(ClientPolicy::new("alpha")).unwrap();
        policy["revoked"] = serde_json::json!(["1.1.0"]);
        write_atomic(&e.dirs.policy_file(), policy.to_string().as_bytes()).unwrap();
        let p = e.launch();
        assert_eq!(p.exe, e.dirs.base_exe());
        assert!(matches!(p.boot, Boot::RolledBack { failed, .. } if failed == v("1.1.0")));
    }

    #[test]
    fn command_forwards_argv_verbatim_and_adds_only_the_root() {
        let e = Env::new("cmd");
        let p = e.launch();
        let fwd: Vec<OsString> = ["--", "mvs://join/abc?x=1", "--inspect"].iter().map(OsString::from).collect();
        let cmd = command(&p, &e.dirs, &fwd);
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(args[0].to_str().unwrap().starts_with(UPDATE_ROOT_ARG));
        assert_eq!(&args[1..], fwd.iter().map(OsString::as_os_str).collect::<Vec<_>>().as_slice());
        let removed: Vec<&OsStr> = cmd.get_envs().filter(|(_, val)| val.is_none()).map(|(k, _)| k).collect();
        assert!(removed.contains(&OsStr::new("NODE_OPTIONS")) && removed.contains(&OsStr::new("ELECTRON_RUN_AS_NODE")));
        assert_eq!(cmd.get_program(), e.dirs.base_exe());
    }
}
