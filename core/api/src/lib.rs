//! The MVS core API: what the UI calls. Mirrors `CoreApi` in
//! `ui/src/core/api.ts` and `ui/src/core/types.ts`, and stays shell-agnostic:
//! a shell (Tauri, Electron, plain IPC) only forwards JSON to
//! [`Core::handle`] and forwards [`Event`]s from its [`EventSink`].
//!
//! In process, no network yet. Wired to `identity` (create, restore, the
//! recovery check before creating a server), `records` + `authority`
//! (servers, roles, kicks) and a local SQLite store. Sync networking and
//! group encryption sit behind [`Network`] and [`GroupCipher`] until their
//! crates are merged.

mod db;
mod dispatch;
mod model;
mod ports;
mod service;
pub mod types;

pub use dispatch::NOT_YET;
pub use ports::{
    CipherOutcome, Clock, CollectingSink, EventSink, FixedClock, GroupCipher, LoopbackNetwork,
    Network, PendingGroupKeys, Ports, SharedSecrets, SystemClock,
};
pub use service::{Config, Core, MIN_CHECK_POSITIONS};
pub use types::*;
