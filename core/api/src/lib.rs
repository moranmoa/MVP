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

mod access;
mod chat;
pub mod crypto;
mod datadir;
mod db;
mod dispatch;
mod gate;
mod model;
mod ports;
mod service;
pub mod sidecar;
pub mod syncnet;
pub mod types;

pub use access::{KeyBinder, MemberReadGate, MemberStubGate, BINDING_PREFIX};
pub use dispatch::NOT_YET;
pub use gate::MemberGate;
pub use ports::{
    CipherError, CipherOutcome, Clock, CollectingSink, EventSink, FixedClock, GroupCipher,
    LoopbackNetwork, Network, Opened, PendingGroupKeys, Ports, SharedSecrets, SystemClock,
};
pub use service::{Config, Core, CHALLENGE_POSITIONS, MAX_CHECK_FAILURES, SYNC_EVERY};
pub use types::*;
