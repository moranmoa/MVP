# B8: the `Updater` facade API (frozen)

This is the Rust API that `core/api` (B8-5, dev-identity) calls. It matches
the JSON in `core/api/CONFORMANCE.md` ("Proposed: in-app update", #163). The
implementation lands in B8-4 under exactly these names. A change to anything
here goes through the tech lead, with dev-identity and dev-voice-shell told
first.

Design: `B8-in-app-update.md` (#101). "Verified" means the existing
`updater` checks (signed chain, `ClientPolicy`, hash, `unpacked_bytes`).

## Construction

```rust
use updater::client::{Updater, UpdaterConfig, HttpsSource};

let cfg = UpdaterConfig {
    dirs: UpdateDirs::new(install_root, UpdateDirs::default_state_dir()?), // from --update-root
    core_data: core_data_dir,         // the folder with core.db (for the snapshot record)
    platform: "windows-x86_64".into(),
    channel: "alpha".into(),          // UI "stable" maps to "alpha" in the closed alpha
    installed: env!("CARGO_PKG_VERSION").parse()?, // the running app version, from the build
    baseline: Baseline { build_time, revoked: vec![] },
};
let updater = Updater::new(cfg, HttpsSource::release(channel));
```

- The trust root is `updater::embedded_root()`; rotations come from the
  source and are applied with the GAP-12 caps.
- `Updater` is `Send + Sync`. Every method takes the state lock
  (`UpdateLock`) for as long as it touches state, so the launcher and the
  core never write at the same time.
- Methods block and do I/O, so the core calls them on a worker thread. The
  6-hour timer is the core's (CONFORMANCE).

## Methods

```rust
impl<S: UpdateSource> Updater<S> {
    /// Fetches root rotations and the channel manifest, verifies them, and
    /// updates policy.json. Never downloads the payload.
    pub fn check(&self, now: u64) -> Result<CheckResult, UpdateError>;

    /// check(), then downloads the payload for this platform (hash + size
    /// checked while writing), unpacks it into versions/<v> and stages it.
    /// Returns the events to emit (UpdateEvent::Ready).
    pub fn download(&self, now: u64) -> Result<Downloaded, UpdateError>;

    /// The user pressed "restart to update". Refused while in a call. On
    /// Ok, main relaunches through the launcher (desktop/src/update.js).
    pub fn apply(&self, in_call: bool) -> Result<Version, UpdateError>;

    /// The UI loaded and the core answered (called by main, never the
    /// renderer: A2). Commits a trial, deletes the DB snapshot, and returns
    /// update.installed with the notes in `ui_lang` (notes_for fallback).
    pub fn mark_healthy(&self, ui_lang: &str) -> Result<Healthy, UpdateError>;

    /// Once at core start: update.rolledBack if the launcher rolled back
    /// since the last start (read from, then cleared in, the state folder).
    pub fn startup_events(&self) -> Vec<UpdateEvent>;

    /// For the core's migration code: records `core.db.pre-<schema>` (a
    /// plain file name in core_data) so the launcher can restore it (A5).
    pub fn record_db_snapshot(&self, file: &str) -> Result<(), UpdateError>;
}
```

## Types

```rust
pub struct CheckResult {
    pub status: CheckStatus,
    pub current: Version,
    pub freshness: Freshness,          // Fresh | Stale | Never (existing)
}
pub enum CheckStatus { UpToDate, Available(Version), Staged(Version) }

pub struct Downloaded { pub version: Version, pub events: Vec<UpdateEvent> }
pub struct Healthy   { pub committed: Option<Version>, pub events: Vec<UpdateEvent> }

pub enum UpdateEvent {
    /// `update.ready` (UpdateInfo)
    Ready { version: Version, waiting_for_call: bool },
    /// `update.installed`; plain text, render as text only
    Installed { version: Version, notes: Option<String> },
    /// `update.rolledBack`
    RolledBack { failed: Version, running: Version },
}
impl UpdateEvent {
    pub fn name(&self) -> &'static str;          // "update.ready" | "update.installed" | "update.rolledBack"
    pub fn payload(&self) -> serde_json::Value;  // exactly the CONFORMANCE JSON (camelCase)
}

pub enum UpdateError {
    Offline,                 // "offline": the source could not be reached
    InvalidUpdate(Error),    // "invalid-update": any verification failure; the detail is for the log only
    CheckClock,              // "check-clock": IssuedInFuture, or nothing unexpired
    NothingToDownload,       // "nothing-to-download"
    NothingStaged,           // "nothing-staged"
    InCall,                  // "not-allowed" (apply during a call)
    NotATrial,               // "not-allowed" (mark_healthy on a committed version)
    DiskFull,                // "disk-full"
    Io(String),              // "internal"
}
impl UpdateError {
    pub fn code(&self) -> &'static str;          // the CONFORMANCE code above
}
```

## The transport trait

```rust
pub trait UpdateSource: Send + Sync {
    /// The signed manifest for `channel`, at most MAX_MANIFEST_BYTES.
    fn manifest(&self, channel: &str) -> Result<Vec<u8>, SourceError>;
    /// Signed root rotations; the caller applies parse_root_fetch (GAP-12).
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError>;
    /// Streams the payload into `out`, stopping after `max_len` bytes.
    fn artifact(&self, version: Version, platform: &str, max_len: u64, out: &mut dyn Write)
        -> Result<(), SourceError>;
}
pub enum SourceError { Offline(String), TooLarge, NotFound, Refused(String) }
```

`HttpsSource::release(channel)` reads
`https://github.com/moranmoa/MVP/releases/download/update-<channel>/`:
`manifest.json`, `root-<n>.json` for n = 2.., and `mvs-<v>-<platform>.zip`.

- TLS through rustls; `https://` only; no cookies; timeouts.
- Redirects only to the exact hosts `github.com`,
  `objects.githubusercontent.com` and `release-assets.githubusercontent.com`
  (A4).
- Every body is capped before it is read in full.
- A later `P2pSource` (after T4) implements the same trait.

## Notes for later (not B8-4)

- Linux tar.gz and macOS payloads use the same entry-name rules as zip,
  plus a check that names don't collide after Unicode normalisation
  (NFC/NFD), since macOS normalises file names (security lead, on #160).
