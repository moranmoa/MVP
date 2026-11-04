//! The `Updater` facade that `core/api` calls (B8-4). The API is frozen in
//! `docs/B8-updater-api.md`; the JSON it maps to is in
//! `core/api/CONFORMANCE.md`.
//!
//! Network and unpacking run without the state lock; every read or write
//! of state takes it, so the launcher is never kept waiting for a download.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::launcher::{record_snapshot, LAUNCH_ATTEMPTS};
use crate::layout::{read_state_file, save_state, write_atomic, UpdateDirs, UpdateLock, APP_EXE};
use crate::manifest::notes_for;
use crate::root::{embedded_root, parse_root_fetch};
use crate::source::{SourceError, UpdateSource};
use crate::unpack::unpack_zip;
use crate::{
    store_artifact, Baseline, ClientPolicy, Decision, Error, Freshness, InstallState, SignedManifest, VerifiedManifest,
    TrustRoot, Version,
};

/// How long a facade call waits for the launcher to release the lock.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// Everything the facade needs to know about this install.
#[derive(Clone, Debug)]
pub struct UpdaterConfig {
    pub dirs: UpdateDirs,
    /// The core's data folder (with `core.db`).
    pub core_data: PathBuf,
    /// e.g. `windows-x86_64`.
    pub platform: String,
    /// Manifest channel (`alpha` while the UI says "stable").
    pub channel: String,
    /// The running app version.
    pub installed: Version,
    pub baseline: Baseline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    UpToDate,
    Available(Version),
    Staged(Version),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub current: Version,
    pub freshness: Freshness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateEvent {
    /// `update.ready`
    Ready { version: Version, waiting_for_call: bool },
    /// `update.installed`. Plain text, rendered as text only.
    Installed { version: Version, notes: Option<String> },
    /// `update.rolledBack`
    RolledBack { failed: Version, running: Version },
}

impl UpdateEvent {
    pub fn name(&self) -> &'static str {
        match self {
            UpdateEvent::Ready { .. } => "update.ready",
            UpdateEvent::Installed { .. } => "update.installed",
            UpdateEvent::RolledBack { .. } => "update.rolledBack",
        }
    }

    /// Exactly the CONFORMANCE.md payload.
    pub fn payload(&self) -> serde_json::Value {
        match self {
            UpdateEvent::Ready { version, waiting_for_call } => {
                json!({ "version": version.to_string(), "waitingForCall": waiting_for_call })
            }
            UpdateEvent::Installed { version, notes } => json!({ "version": version.to_string(), "notes": notes }),
            UpdateEvent::RolledBack { failed, running } => {
                json!({ "failed": failed.to_string(), "running": running.to_string() })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub version: Version,
    pub events: Vec<UpdateEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Healthy {
    pub committed: Option<Version>,
    pub events: Vec<UpdateEvent>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError {
    Offline,
    /// Any verification failure. The detail is for the log, never the UI.
    InvalidUpdate(Error),
    CheckClock,
    NothingToDownload,
    NothingStaged,
    InCall,
    NotATrial,
    DiskFull,
    Io(String),
}

impl UpdateError {
    /// The CONFORMANCE.md error code.
    pub fn code(&self) -> &'static str {
        match self {
            UpdateError::Offline => "offline",
            UpdateError::InvalidUpdate(_) => "invalid-update",
            UpdateError::CheckClock => "check-clock",
            UpdateError::NothingToDownload => "nothing-to-download",
            UpdateError::NothingStaged => "nothing-staged",
            UpdateError::InCall | UpdateError::NotATrial => "not-allowed",
            UpdateError::DiskFull => "disk-full",
            UpdateError::Io(_) => "internal",
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::InvalidUpdate(e) => write!(f, "invalid-update: {e}"),
            UpdateError::Io(e) => write!(f, "internal: {e}"),
            other => f.write_str(other.code()),
        }
    }
}

impl From<SourceError> for UpdateError {
    fn from(e: SourceError) -> Self {
        match e {
            SourceError::Offline(_) => UpdateError::Offline,
            // Too large, missing or refused files from the update host are
            // not something the user can fix: report them as invalid.
            other => UpdateError::InvalidUpdate(Error::Malformed(other.to_string())),
        }
    }
}

/// Verification errors that mean "your clock is off".
fn from_verify(e: Error) -> UpdateError {
    match e {
        Error::IssuedInFuture { .. } | Error::Expired { .. } => UpdateError::CheckClock,
        other => UpdateError::InvalidUpdate(other),
    }
}

fn io_err(e: Error) -> UpdateError {
    match e {
        Error::Io(why) => UpdateError::Io(why),
        other => UpdateError::InvalidUpdate(other),
    }
}

/// `staged-notes.json`: the staged version's signed notes.
#[derive(Serialize, Deserialize)]
struct StagedNotes {
    version: Version,
    notes: BTreeMap<String, String>,
}

pub struct Updater<S: UpdateSource> {
    cfg: UpdaterConfig,
    source: S,
    root: TrustRoot,
}

impl<S: UpdateSource> Updater<S> {
    /// Trusts `embedded_root()` (the shipped root) plus the signed chain.
    pub fn new(cfg: UpdaterConfig, source: S) -> Self {
        Updater::with_root(cfg, source, embedded_root())
    }

    /// Like `new`, starting from another root: tests and tools only, never
    /// the app.
    pub fn with_root(cfg: UpdaterConfig, source: S, root: TrustRoot) -> Self {
        Updater { cfg, source, root }
    }

    fn lock(&self) -> Result<UpdateLock, UpdateError> {
        UpdateLock::acquire(&self.cfg.dirs, LOCK_WAIT).map_err(io_err)
    }

    /// The state as the launcher left it. Only the launcher knows the
    /// installer's base version, so the core never re-bases: it uses the
    /// saved state as is (validated on parse), or a fresh one on the
    /// running version if there is none.
    fn load_state(&self) -> InstallState {
        read_state_file(&self.cfg.dirs.state_file())
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<InstallState>(&b).ok())
            .filter(|s| s.platform() == self.cfg.platform)
            .unwrap_or_else(|| InstallState::new(self.cfg.installed, &self.cfg.platform, LAUNCH_ATTEMPTS))
    }

    fn save_state(&self, state: &InstallState) -> Result<(), UpdateError> {
        let parses = |b: &[u8]| serde_json::from_slice::<InstallState>(b).is_ok();
        save_state(&self.cfg.dirs, state.to_json().as_bytes(), parses).map_err(io_err)
    }

    fn load_policy(&self) -> ClientPolicy {
        let saved = read_state_file(&self.cfg.dirs.policy_file()).ok().flatten();
        ClientPolicy::restore(saved.as_deref(), &self.cfg.channel, self.cfg.installed, &self.cfg.baseline).0
    }

    /// Fetches and verifies; the policy is updated on disk.
    fn fetch_verified(&self, now: u64) -> Result<(Option<VerifiedManifest>, CheckResult), UpdateError> {
        let rotations = parse_root_fetch(&self.source.root_updates()?).map_err(UpdateError::InvalidUpdate)?;
        let chain = self.root.update_chain(&rotations);
        if let Some(v) = chain.fork_at {
            return Err(UpdateError::InvalidUpdate(Error::InvalidTrustRoot(format!("two rotations to v{v}"))));
        }
        let fetched = match self.source.manifest(&self.cfg.channel) {
            Ok(bytes) => Some(bytes),
            // Nothing published on this channel yet.
            Err(SourceError::NotFound) => None,
            Err(e) => return Err(e.into()),
        };

        let _lock = self.lock()?;
        let mut policy = self.load_policy();
        let state = self.load_state();
        let Some(bytes) = fetched else {
            return Ok((None, self.result(CheckStatus::UpToDate, &policy, now)));
        };
        let manifest = SignedManifest::from_json(&bytes)
            .and_then(|m| m.verify(&chain.root, now))
            .map_err(from_verify)?;
        let decision = policy.accept(&manifest, self.cfg.installed).map_err(from_verify)?;
        write_atomic(&self.cfg.dirs.policy_file(), policy.to_json().as_bytes()).map_err(io_err)?;
        let status = match decision {
            Decision::UpToDate => CheckStatus::UpToDate,
            Decision::Install(v) if state.trial().is_some_and(|t| t.version == v) => CheckStatus::Staged(v),
            Decision::Install(v) => CheckStatus::Available(v),
        };
        Ok((Some(manifest), self.result(status, &policy, now)))
    }

    fn result(&self, status: CheckStatus, policy: &ClientPolicy, now: u64) -> CheckResult {
        CheckResult { status, current: self.cfg.installed, freshness: policy.freshness(now) }
    }

    /// Fetches root rotations and the channel manifest, verifies them, and
    /// updates `policy.json`. Never downloads the payload.
    pub fn check(&self, now: u64) -> Result<CheckResult, UpdateError> {
        self.fetch_verified(now).map(|(_, r)| r)
    }

    /// `check`, then downloads, verifies, unpacks and stages the payload.
    pub fn download(&self, now: u64) -> Result<Downloaded, UpdateError> {
        let (manifest, result) = self.fetch_verified(now)?;
        let version = match result.status {
            CheckStatus::UpToDate => return Err(UpdateError::NothingToDownload),
            CheckStatus::Staged(v) => return Ok(ready(v)),
            CheckStatus::Available(v) => v,
        };
        let manifest = manifest.expect("a version is available only from a manifest");
        let entry = manifest
            .body()
            .artifacts
            .get(&self.cfg.platform)
            .ok_or_else(|| UpdateError::InvalidUpdate(Error::NoArtifact(self.cfg.platform.clone())))?;

        // 1. Download into downloads/, capped at the signed size.
        let downloads = self.cfg.dirs.downloads();
        fs::create_dir_all(&downloads).map_err(|e| self.disk(e))?;
        let incoming = downloads.join(format!("{version}-{}.part", self.cfg.platform));
        let mut out = DiskWriter::create(&incoming).map_err(|e| self.disk(e))?;
        let fetched = self.source.artifact(version, &self.cfg.platform, entry.size, &mut out);
        let full = out.full;
        drop(out);
        if let Err(e) = fetched {
            let _ = fs::remove_file(&incoming);
            return Err(if full { UpdateError::DiskFull } else { e.into() });
        }

        // 2. Verify (hash while copying), then unpack from the verified handle.
        let file = File::open(&incoming).map_err(|e| self.disk(e))?;
        let stored = store_artifact(&manifest, &self.cfg.platform, file, &downloads);
        let _ = fs::remove_file(&incoming);
        let stored = stored.map_err(io_err)?;
        let unpacked = (|| {
            let verified = stored.open_verified()?;
            let dir = self.cfg.dirs.version_dir(version);
            if dir.exists() {
                // Left over from an earlier attempt that was never staged.
                fs::remove_dir_all(&dir)?;
            }
            unpack_zip(verified, stored.artifact(), &self.cfg.dirs.versions(), APP_EXE)
        })();
        let _ = fs::remove_file(stored.path());
        unpacked.map_err(|e| match e {
            Error::Io(why) if is_disk_full_text(&why) => UpdateError::DiskFull,
            other => io_err(other),
        })?;

        // 3. Stage, and keep the signed notes for update.installed.
        let _lock = self.lock()?;
        let mut state = self.load_state();
        state.stage(stored.artifact(), self.load_policy().revoked()).map_err(UpdateError::InvalidUpdate)?;
        self.save_state(&state)?;
        let notes = StagedNotes { version, notes: manifest.body().notes.clone() };
        let json = serde_json::to_vec(&notes).expect("notes serialise");
        write_atomic(&self.cfg.dirs.staged_notes(), &json).map_err(io_err)?;
        Ok(ready(version))
    }

    /// The user pressed "restart to update". On Ok, main relaunches through
    /// the launcher.
    pub fn apply(&self, in_call: bool) -> Result<Version, UpdateError> {
        let _lock = self.lock()?;
        let state = self.load_state();
        let Some(trial) = state.trial() else { return Err(UpdateError::NothingStaged) };
        if in_call {
            return Err(UpdateError::InCall);
        }
        Ok(trial.version)
    }

    /// The UI loaded and the core answered (main only, A2). Commits the
    /// running trial version, drops the DB snapshot and the old version's
    /// folder, and returns `update.installed` with the notes in `ui_lang`.
    pub fn mark_healthy(&self, ui_lang: &str) -> Result<Healthy, UpdateError> {
        let _lock = self.lock()?;
        let mut state = self.load_state();
        let running = self.cfg.installed;
        match state.trial() {
            Some(t) if t.version == running && t.attempts > 0 => {}
            _ if state.current() == running => return Ok(Healthy { committed: None, events: vec![] }),
            _ => return Err(UpdateError::NotATrial),
        }
        let committed = state.report_healthy().map_err(|_| UpdateError::NotATrial)?;
        self.save_state(&state)?;

        // The snapshot only protected the rollback that can no longer happen.
        if let Ok(Some(b)) = read_state_file(&self.cfg.dirs.snapshot_file()) {
            if let Ok(rec) = serde_json::from_slice::<serde_json::Value>(&b) {
                if let Some(file) = rec["file"].as_str().filter(|f| f.starts_with("core.db.pre-") && !f.contains(['/', '\\'])) {
                    let _ = fs::remove_file(self.cfg.core_data.join(file));
                }
            }
            let _ = fs::remove_file(self.cfg.dirs.snapshot_file());
        }
        if let Some(old) = committed.discard {
            let _ = fs::remove_dir_all(self.cfg.dirs.version_dir(old));
        }

        let notes = read_state_file(&self.cfg.dirs.staged_notes())
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<StagedNotes>(&b).ok())
            .filter(|n| n.version == running)
            .and_then(|n| notes_for(&n.notes, ui_lang).map(str::to_string));
        let _ = fs::remove_file(self.cfg.dirs.staged_notes());
        Ok(Healthy {
            committed: Some(committed.version),
            events: vec![UpdateEvent::Installed { version: committed.version, notes }],
        })
    }

    /// Once at core start: `update.rolledBack` if the launcher rolled back
    /// since the last start.
    pub fn startup_events(&self) -> Vec<UpdateEvent> {
        let Ok(_lock) = self.lock() else { return vec![] };
        let path = self.cfg.dirs.rollback_notice();
        let event = read_state_file(&path).ok().flatten().and_then(|b| {
            let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
            Some(UpdateEvent::RolledBack {
                failed: v["failed"].as_str()?.parse().ok()?,
                running: v["running"].as_str()?.parse().ok()?,
            })
        });
        let _ = fs::remove_file(path);
        event.into_iter().collect()
    }

    /// For the core's migration code: records `core.db.pre-<schema>` (a
    /// plain file name in `core_data`) so the launcher can restore it (A5).
    pub fn record_db_snapshot(&self, file: &str) -> Result<(), UpdateError> {
        let _lock = self.lock()?;
        record_snapshot(&self.cfg.dirs, self.cfg.installed, file).map_err(io_err)
    }

    fn disk(&self, e: io::Error) -> UpdateError {
        if e.kind() == io::ErrorKind::StorageFull {
            UpdateError::DiskFull
        } else {
            UpdateError::Io(e.to_string())
        }
    }
}

fn ready(version: Version) -> Downloaded {
    Downloaded { version, events: vec![UpdateEvent::Ready { version, waiting_for_call: false }] }
}

fn is_disk_full_text(why: &str) -> bool {
    let w = why.to_ascii_lowercase();
    w.contains("no space") || w.contains("disk full") || w.contains("not enough space")
}

/// A file writer that remembers whether the disk filled up.
struct DiskWriter {
    file: File,
    full: bool,
}

impl DiskWriter {
    fn create(path: &std::path::Path) -> io::Result<Self> {
        Ok(DiskWriter { file: File::create(path)?, full: false })
    }
}

impl Write for DiskWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf).inspect_err(|e| self.full |= e.kind() == io::ErrorKind::StorageFull)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::plan;
    use crate::manifest::tests::{key, signed_by, NOW};
    use crate::{Artifact, Boot, ManifestBody, SigningKey, FORMAT};
    use std::io::Cursor;
    use std::sync::Mutex;

    const PLATFORM: &str = "windows-x86_64";

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    /// An in-memory source; `offline` makes every call fail like no network.
    #[derive(Default)]
    struct Mock {
        manifest: Option<Vec<u8>>,
        payload: Vec<u8>,
        offline: bool,
        artifact_calls: Mutex<u32>,
    }

    impl UpdateSource for Mock {
        fn manifest(&self, _channel: &str) -> Result<Vec<u8>, SourceError> {
            if self.offline {
                return Err(SourceError::Offline("test".into()));
            }
            self.manifest.clone().ok_or(SourceError::NotFound)
        }
        fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError> {
            if self.offline {
                return Err(SourceError::Offline("test".into()));
            }
            Ok(vec![])
        }
        fn artifact(&self, _v: Version, _p: &str, max_len: u64, out: &mut dyn Write) -> Result<(), SourceError> {
            *self.artifact_calls.lock().unwrap() += 1;
            if self.payload.len() as u64 > max_len {
                return Err(SourceError::TooLarge);
            }
            out.write_all(&self.payload).map_err(|e| SourceError::Offline(e.to_string()))
        }
    }

    struct World {
        root: PathBuf,
        dirs: UpdateDirs,
        data: PathBuf,
        key: SigningKey,
    }

    impl World {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("mvs-client-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let dirs = UpdateDirs::new(root.join("install"), root.join("state"));
            fs::create_dir_all(dirs.install()).unwrap();
            fs::write(dirs.base_exe(), b"base").unwrap();
            let data = root.join("data");
            fs::create_dir_all(&data).unwrap();
            World { root, dirs, data, key: key() }
        }

        fn updater(&self, installed: &str, source: Mock) -> Updater<Mock> {
            let cfg = UpdaterConfig {
                dirs: self.dirs.clone(),
                core_data: self.data.clone(),
                platform: PLATFORM.into(),
                channel: "alpha".into(),
                installed: v(installed),
                baseline: Baseline::default(),
            };
            Updater::with_root(cfg, source, TrustRoot::single(self.key.verifying_key()).unwrap())
        }

        /// A signed 1.1.0 manifest for `payload`, plus a source serving it.
        fn release(&self, payload: Vec<u8>, unpacked: u64) -> Mock {
            let (sha256, size) = crate::hash_reader(Cursor::new(&payload)).unwrap();
            let mut artifacts = BTreeMap::new();
            artifacts.insert(PLATFORM.to_string(), Artifact { sha256, size, unpacked_bytes: Some(unpacked) });
            let mut notes = BTreeMap::new();
            notes.insert("he".to_string(), "תיקונים".to_string());
            notes.insert("en".to_string(), "Fixes".to_string());
            let body = ManifestBody {
                format: FORMAT,
                channel: "alpha".into(),
                version: v("1.1.0"),
                issued_at: NOW - 60,
                expires_at: NOW + 86_400,
                artifacts,
                revoked: Default::default(),
                notes,
            };
            let manifest = signed_by(&body, &[&self.key]).to_json().into_bytes();
            Mock { manifest: Some(manifest), payload, ..Mock::default() }
        }

        fn launch(&self) -> Boot {
            plan(&self.dirs, &self.data, v("1.0.0"), PLATFORM).unwrap().boot
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn payload() -> (Vec<u8>, u64) {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file(APP_EXE, opts).unwrap();
        w.write_all(b"new app").unwrap();
        w.start_file("resources/app.asar", opts).unwrap();
        w.write_all(b"asar").unwrap();
        (w.finish().unwrap().into_inner(), 11)
    }

    #[test]
    fn check_download_apply_launch_and_mark_healthy() {
        let w = World::new("flow");
        let (zip, unpacked) = payload();
        let up = w.updater("1.0.0", w.release(zip.clone(), unpacked));

        let r = up.check(NOW).unwrap();
        assert_eq!((r.status, r.current, r.freshness), (CheckStatus::Available(v("1.1.0")), v("1.0.0"), Freshness::Fresh));
        assert_eq!(up.apply(false), Err(UpdateError::NothingStaged));

        let d = up.download(NOW).unwrap();
        assert_eq!(d.events, vec![UpdateEvent::Ready { version: v("1.1.0"), waiting_for_call: false }]);
        assert_eq!(fs::read(w.dirs.version_exe(v("1.1.0"))).unwrap(), b"new app");
        assert_eq!(up.check(NOW).unwrap().status, CheckStatus::Staged(v("1.1.0")));
        // A second download is a no-op.
        assert_eq!(up.download(NOW).unwrap().version, v("1.1.0"));
        assert_eq!(*up.source.artifact_calls.lock().unwrap(), 1);

        assert_eq!(up.apply(true), Err(UpdateError::InCall));
        assert_eq!(up.apply(false), Ok(v("1.1.0")));
        // The old version is still running: nothing to commit.
        assert_eq!(up.mark_healthy("he").unwrap().committed, None);

        // Relaunch: the launcher starts the trial, which reports healthy.
        assert_eq!(w.launch(), Boot::Trial(v("1.1.0")));
        let new = w.updater("1.1.0", w.release(zip, unpacked));
        new.record_db_snapshot("core.db.pre-7").unwrap();
        fs::write(w.data.join("core.db.pre-7"), b"old").unwrap();
        let h = new.mark_healthy("he-IL").unwrap();
        assert_eq!(h.committed, Some(v("1.1.0")));
        assert_eq!(h.events, vec![UpdateEvent::Installed { version: v("1.1.0"), notes: Some("תיקונים".into()) }]);
        assert!(!w.data.join("core.db.pre-7").exists() && !w.dirs.snapshot_file().exists());
        assert_eq!(w.launch(), Boot::Normal(v("1.1.0")));
        assert_eq!(new.check(NOW).unwrap().status, CheckStatus::UpToDate);
    }

    #[test]
    fn tampered_payload_is_refused_and_nothing_is_staged() {
        let w = World::new("tamper");
        let (zip, unpacked) = payload();
        let mut src = w.release(zip, unpacked);
        let last = src.payload.len() - 1;
        src.payload[last] ^= 1;
        let up = w.updater("1.0.0", src);
        assert!(matches!(up.download(NOW), Err(UpdateError::InvalidUpdate(_))));
        assert!(!w.dirs.version_dir(v("1.1.0")).exists());
        assert_eq!(up.apply(false), Err(UpdateError::NothingStaged));
        assert_eq!(w.launch(), Boot::Normal(v("1.0.0")));
    }

    #[test]
    fn manifest_signed_by_another_key_is_invalid() {
        let w = World::new("forged");
        let other = World::new("forged-signer");
        let (zip, unpacked) = payload();
        let err = w.updater("1.0.0", other.release(zip, unpacked)).check(NOW).unwrap_err();
        assert_eq!(err.code(), "invalid-update");
    }

    #[test]
    fn offline_nothing_published_and_clock_errors_map_to_codes() {
        let w = World::new("codes");
        let off = w.updater("1.0.0", Mock { offline: true, ..Mock::default() });
        assert_eq!(off.check(NOW).unwrap_err().code(), "offline");
        let none = w.updater("1.0.0", Mock::default()).check(NOW).unwrap();
        assert_eq!((none.status, none.freshness), (CheckStatus::UpToDate, Freshness::Never));
        assert_eq!(w.updater("1.0.0", Mock::default()).download(NOW), Err(UpdateError::NothingToDownload));
        let (zip, unpacked) = payload();
        let up = w.updater("1.0.0", w.release(zip, unpacked));
        assert_eq!(up.check(NOW + 2 * 86_400).unwrap_err().code(), "check-clock");
        assert_eq!(up.check(NOW - 3 * 86_400).unwrap_err().code(), "check-clock");
    }

    #[test]
    fn rollback_is_announced_once_at_startup() {
        let w = World::new("rolledback");
        let (zip, unpacked) = payload();
        let up = w.updater("1.0.0", w.release(zip, unpacked));
        up.download(NOW).unwrap();
        for _ in 0..LAUNCH_ATTEMPTS {
            assert_eq!(w.launch(), Boot::Trial(v("1.1.0")));
        }
        assert!(matches!(w.launch(), Boot::RolledBack { .. }));
        assert_eq!(up.startup_events(), vec![UpdateEvent::RolledBack { failed: v("1.1.0"), running: v("1.0.0") }]);
        assert_eq!(up.startup_events(), vec![]);
    }

    #[test]
    fn event_payloads_match_conformance() {
        let ready = UpdateEvent::Ready { version: v("1.3.0"), waiting_for_call: true };
        assert_eq!((ready.name(), ready.payload()), ("update.ready", json!({ "version": "1.3.0", "waitingForCall": true })));
        let inst = UpdateEvent::Installed { version: v("1.3.0"), notes: None };
        assert_eq!((inst.name(), inst.payload()), ("update.installed", json!({ "version": "1.3.0", "notes": null })));
        let back = UpdateEvent::RolledBack { failed: v("1.3.0"), running: v("1.2.0") };
        assert_eq!((back.name(), back.payload()), ("update.rolledBack", json!({ "failed": "1.3.0", "running": "1.2.0" })));
        assert_eq!(UpdateError::InCall.code(), "not-allowed");
        assert_eq!(UpdateError::DiskFull.code(), "disk-full");
    }
}
