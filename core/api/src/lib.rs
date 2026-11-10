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
pub mod invite_link;
mod model;
mod ports;
mod rekey;
mod service;
pub mod sidecar;
pub mod syncnet;
pub mod types;
pub mod update;
mod voice;

pub use access::{KeyBinder, MemberReadGate, MemberStubGate, BINDING_PREFIX};
pub use dispatch::NOT_YET;
pub use gate::MemberGate;
pub use ports::{
    CipherError, CipherOutcome, Clock, CollectingSink, EventSink, FixedClock, GroupCipher,
    LoopbackNetwork, Network, Opened, PendingGroupKeys, Ports, Reach, SharedSecrets, SystemClock,
};
pub use rekey::{JOIN_REKEY_BURST, JOIN_REKEY_REFILL_MS};
pub use service::stored_relay_only;
pub use service::{
    Config, Core, CHALLENGE_POSITIONS, CLOSED_RETENTION_MS, JOIN_RETRY_FIRST, JOIN_RETRY_FOR,
    JOIN_RETRY_MAX, MAX_CHECK_FAILURES, REKEY_WAKE_FLOOR, SYNC_EVERY,
};
pub use types::*;
pub use voice::{
    VoiceNet, CLOCK_WINDOW_MS, HELLO_EVERY_MS, HELLO_TIMEOUT_MS, MAX_PARTICIPANTS, MAX_PAYLOAD,
    SIGNAL_PREFIX,
};
