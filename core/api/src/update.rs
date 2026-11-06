//! In-app update (B8-5): the `updater` crate's `Updater` facade wired into
//! the core. The JSON is `CONFORMANCE.md` "Proposed: in-app update (B8)";
//! every check (signatures, root chain, policy, hash, size, unpack) stays in
//! `updater`. This module only maps channels, codes and events, and runs
//! the background check.
//!
//! Updates are on only when the shell passes `--update-root` (a packaged app
//! whose root checks out) and the binary knows its own version. Otherwise
//! [`Core`](crate::Core) has no `Updates`, the four commands answer
//! `not-implemented`, and nothing here touches the network.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use updater::client::{CheckStatus, UpdateError, UpdateEvent, Updater, UpdaterConfig};
use updater::source::{SourceError, UpdateSource};
use updater::{Baseline, Freshness, TrustRoot, UpdateDirs, Version};

use crate::types::{CoreError, CoreResult, ErrorCode, UpdateChannel};

/// Between background checks (design §3: every 6 h ± 30 min).
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
pub const CHECK_JITTER: Duration = Duration::from_secs(30 * 60);
/// The first check waits for `markHealthy` (the UI is up and hears
/// `update.ready`), or this long when it never comes.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(2 * 60);

/// The manifest channel for a UI channel. During the closed alpha the UI's
/// `stable` is the `alpha` manifest (§0, devops); `beta` gets every
/// pre-release.
pub fn manifest_channel(ui: UpdateChannel) -> &'static str {
    match ui {
        UpdateChannel::Stable => "alpha",
        UpdateChannel::Beta => "beta",
    }
}

/// Builds the transport for a manifest channel: `HttpsSource::release` in
/// the app, an in-memory source in tests.
pub type SourceFor = Arc<dyn Fn(&str) -> Result<Box<dyn UpdateSource>, SourceError> + Send + Sync>;

/// A boxed source, so one `Updater` type serves every transport.
pub struct DynSource(Box<dyn UpdateSource>);

impl UpdateSource for DynSource {
    fn manifest(&self, channel: &str) -> Result<Vec<u8>, SourceError> {
        self.0.manifest(channel)
    }
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError> {
        self.0.root_updates()
    }
    fn artifact(&self, version: Version, platform: &str, max_len: u64, out: &mut dyn Write) -> Result<(), SourceError> {
        self.0.artifact(version, platform, max_len, out)
    }
}

/// Everything about this install except the channel, which the user picks.
pub struct UpdateSetup {
    pub dirs: UpdateDirs,
    /// The core's data folder (with `core.db`).
    pub core_data: PathBuf,
    /// `<os>-<arch>`, as the launcher and `manifest-body.mjs` name it.
    pub platform: String,
    /// The running app version.
    pub installed: Version,
    pub baseline: Baseline,
    /// `None` in the app: the shipped root (`embedded_root`). Tests only.
    pub root: Option<TrustRoot>,
    pub source: SourceFor,
}

/// The `Updater` for the current channel, rebuilt when the channel changes.
pub struct Updates {
    setup: UpdateSetup,
    current: Mutex<Option<(&'static str, Arc<Updater<DynSource>>)>>,
    /// One download at a time across channel switches (each `Updater` only
    /// serialises its own).
    downloading: Mutex<()>,
    /// `markHealthy` was handled (the UI is up); `startup_events` sent.
    healthy_seen: Mutex<bool>,
    wake: (Mutex<Wake>, Condvar),
}

#[derive(Default)]
struct Wake {
    pending: bool,
    stop: bool,
}

/// Results as `CONFORMANCE.md` shapes them, plus the events to emit.
pub struct Outcome {
    pub ok: Value,
    pub events: Vec<UpdateEvent>,
}

impl Updates {
    pub fn new(setup: UpdateSetup) -> Updates {
        Updates {
            setup,
            current: Mutex::new(None),
            downloading: Mutex::new(()),
            healthy_seen: Mutex::new(false),
            wake: (Mutex::new(Wake::default()), Condvar::new()),
        }
    }

    pub fn installed(&self) -> Version {
        self.setup.installed
    }

    fn updater(&self, ui: UpdateChannel) -> CoreResult<Arc<Updater<DynSource>>> {
        let channel = manifest_channel(ui);
        let mut current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((c, up)) = current.as_ref() {
            if *c == channel {
                return Ok(up.clone());
            }
        }
        let source = (self.setup.source)(channel).map_err(|e| code(&UpdateError::from(e)))?;
        let cfg = UpdaterConfig {
            dirs: self.setup.dirs.clone(),
            core_data: self.setup.core_data.clone(),
            platform: self.setup.platform.clone(),
            channel: channel.to_string(),
            installed: self.setup.installed,
            baseline: self.setup.baseline.clone(),
        };
        let source = DynSource(source);
        let up = Arc::new(match &self.setup.root {
            Some(root) => Updater::with_root(cfg, source, root.clone()),
            None => Updater::new(cfg, source),
        });
        *current = Some((channel, up.clone()));
        Ok(up)
    }

    /// `checkForUpdate`. `now` is Unix seconds.
    pub fn check(&self, ui: UpdateChannel, now: u64) -> CoreResult<Value> {
        let r = self.updater(ui)?.check(now).map_err(|e| logged("check", e))?;
        let (status, available) = match r.status {
            CheckStatus::UpToDate => ("up-to-date", None),
            CheckStatus::Available(v) => ("available", Some(v.to_string())),
            CheckStatus::Staged(v) => ("staged", Some(v.to_string())),
        };
        let freshness = match r.freshness {
            Freshness::Fresh => "fresh",
            Freshness::Stale { .. } => "stale",
            Freshness::Never => "never",
        };
        Ok(json!({
            "status": status,
            "current": r.current.to_string(),
            "available": available,
            "freshness": freshness,
        }))
    }

    /// `downloadUpdate`: verified, unpacked and staged, or an error.
    pub fn download(&self, ui: UpdateChannel, now: u64) -> CoreResult<Outcome> {
        let up = self.updater(ui)?;
        let _one = self.downloading.lock().unwrap_or_else(|p| p.into_inner());
        let d = up.download(now).map_err(|e| logged("download", e))?;
        Ok(Outcome { ok: json!({ "version": d.version.to_string() }), events: d.events })
    }

    /// `applyUpdate`. Voice is phase 2: `in_call` is always false today.
    pub fn apply(&self, ui: UpdateChannel, in_call: bool) -> CoreResult<Value> {
        let v = self.updater(ui)?.apply(in_call).map_err(|e| logged("apply", e))?;
        Ok(json!({ "version": v.to_string() }))
    }

    /// `markHealthy` (main only, A2). The first call also carries
    /// `update.rolledBack` when the launcher rolled back, and lets the
    /// first background check run (the UI now hears `update.ready`).
    pub fn mark_healthy(&self, ui: UpdateChannel, lang: &str) -> CoreResult<Outcome> {
        let up = self.updater(ui)?;
        let h = up.mark_healthy(lang).map_err(|e| logged("markHealthy", e));
        let mut events = Vec::new();
        {
            let mut seen = self.healthy_seen.lock().unwrap_or_else(|p| p.into_inner());
            if !*seen {
                *seen = true;
                events.extend(up.startup_events());
                self.wake();
            }
        }
        let h = h?;
        events.extend(h.events);
        let committed = h.committed.map(|v| v.to_string());
        Ok(Outcome { ok: json!({ "committed": committed }), events })
    }

    /// One background round: check and download whatever is available.
    /// Errors are logged, never shown (the UI only hears `update.ready`).
    pub fn background_round(&self, ui: UpdateChannel, now: u64) -> Vec<UpdateEvent> {
        match self.download(ui, now) {
            Ok(o) => o.events,
            Err(_) => vec![],
        }
    }

    /// The next background check is due now (a channel switch, the first
    /// `markHealthy`).
    pub fn wake(&self) {
        let (lock, cvar) = &self.wake;
        lock.lock().unwrap_or_else(|p| p.into_inner()).pending = true;
        cvar.notify_all();
    }

    pub fn stop(&self) {
        let (lock, cvar) = &self.wake;
        lock.lock().unwrap_or_else(|p| p.into_inner()).stop = true;
        cvar.notify_all();
    }

    /// Waits up to `timeout` or until woken. False once stopped.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &self.wake;
        let guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let (mut guard, _) = cvar
            .wait_timeout_while(guard, timeout, |w| !w.pending && !w.stop)
            .unwrap_or_else(|p| p.into_inner());
        guard.pending = false;
        !guard.stop
    }
}

/// `CHECK_EVERY` ± `CHECK_JITTER`, so clients don't all ask at once.
pub fn next_interval() -> Duration {
    let mut b = [0u8; 8];
    // A failed RNG only loses the spread, never the check.
    let _ = getrandom(&mut b);
    let span = 2 * CHECK_JITTER.as_secs() + 1;
    let offset = u64::from_le_bytes(b) % span;
    CHECK_EVERY - CHECK_JITTER + Duration::from_secs(offset)
}

fn getrandom(buf: &mut [u8]) -> Result<(), rand_core::Error> {
    use rand_core::RngCore;
    rand_core::OsRng.try_fill_bytes(buf)
}

/// The update error codes (`CONFORMANCE.md`).
fn code(e: &UpdateError) -> CoreError {
    let code = match e {
        UpdateError::Offline => ErrorCode::Offline,
        UpdateError::InvalidUpdate(_) => ErrorCode::InvalidUpdate,
        UpdateError::CheckClock => ErrorCode::CheckClock,
        UpdateError::NothingToDownload => ErrorCode::NothingToDownload,
        UpdateError::NothingStaged => ErrorCode::NothingStaged,
        UpdateError::InCall | UpdateError::NotATrial => ErrorCode::NotAllowed,
        UpdateError::DiskFull => ErrorCode::DiskFull,
        UpdateError::Io(_) => ErrorCode::Internal,
    };
    code.into()
}

/// Details (which check failed) go to the log only, never to the UI.
fn logged(what: &str, e: UpdateError) -> CoreError {
    match e {
        UpdateError::NothingToDownload | UpdateError::NothingStaged | UpdateError::NotATrial => {}
        _ => tracing::warn!("update {what}: {e}"),
    }
    code(&e)
}
