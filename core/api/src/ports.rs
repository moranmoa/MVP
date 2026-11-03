//! What the core needs from the outside, as traits, so the shell (Tauri,
//! Electron, tests) and not-yet-merged crates plug in without touching the
//! core logic.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use identity::{SecretStore, StoreError};
use records::{PublicKey, Record, ServerId};
use zeroize::Zeroizing;

use crate::types::Event;

/// Receives every event, in order. Called after the command's state change
/// is stored, never while the core holds its lock, so a sink may call back
/// into the core.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &Event);
}

/// Keeps events in memory; for tests and for shells that poll.
#[derive(Default)]
pub struct CollectingSink {
    events: Mutex<Vec<Event>>,
}

impl CollectingSink {
    pub fn new() -> CollectingSink {
        CollectingSink::default()
    }

    /// Everything emitted since the last call.
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().expect("not poisoned"))
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().expect("not poisoned").push(event.clone());
    }
}

/// Milliseconds since the Unix epoch.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }
}

/// A clock the tests set by hand.
pub struct FixedClock(pub Mutex<u64>);

impl FixedClock {
    pub fn new(ms: u64) -> FixedClock {
        FixedClock(Mutex::new(ms))
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        let mut now = self.0.lock().expect("not poisoned");
        *now += 1;
        *now
    }
}

/// Moving records between peers.
///
/// TODO(sync, PR #5): adapter over `sync` (libp2p pull by have-markers plus
/// gossipsub push). Until then only [`LoopbackNetwork`] exists, which
/// connects cores inside one process.
///
/// For that adapter: `sync`'s Node drops events when its channel is full, so
/// an event is a hint, never the data. Treat a gap (a sequence number that
/// skips, or a `Lagged` event) as "re-read the store", and `Record::decode`
/// every record at ingest (GAP-1) before it reaches the core.
///
/// For sync's `Authorizer` (#55): `ControlLog` decodes the whole control log
/// for every channel record, so cache the monotone verdict ("owner, or has
/// a stored Join") per author per server, and drop the cache when a new
/// control record arrives.
pub trait Network: Send + Sync {
    /// Offers a record we made or accepted to peers of its server.
    fn publish(&self, record: &Record);
    /// Every record of `server` that reachable peers hold. Empty when nobody
    /// is online.
    fn fetch(&self, server: ServerId) -> Vec<Record>;
}

/// All cores sharing one instance see each other's records instantly.
#[derive(Default)]
pub struct LoopbackNetwork {
    records: Mutex<HashMap<ServerId, Vec<Record>>>,
}

impl LoopbackNetwork {
    pub fn new() -> LoopbackNetwork {
        LoopbackNetwork::default()
    }
}

impl Network for LoopbackNetwork {
    fn publish(&self, record: &Record) {
        let mut records = self.records.lock().expect("not poisoned");
        let list = records.entry(record.server()).or_default();
        if !list.iter().any(|r| r.id() == record.id()) {
            list.push(record.clone());
        }
    }

    fn fetch(&self, server: ServerId) -> Vec<Record> {
        self.records.lock().expect("not poisoned").get(&server).cloned().unwrap_or_default()
    }
}

/// What happened to a server's encryption after a membership change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CipherOutcome {
    Unchanged,
    /// A new key epoch excludes removed members: the UI shows a notice.
    Renewed,
}

/// Per-server group encryption.
///
/// TODO(groupkeys, PR #9): adapter over `groupkeys::ChannelKeys`: rotate on
/// kick, wrap for members, encrypt and decrypt channel messages. Messages
/// aren't implemented in this core yet, so nothing is encrypted or sent in
/// the clear today. When they are: a decrypted message whose
/// `Decrypted.accepted` is false is shown as untrusted, never as a normal
/// message (issue #21). Rotations are deny-by-default: feed
/// `ChannelKeys::set_accepted` / `accept` with `authority::State::accepted` ∩
/// the channel's rotation IDs (GAP-5). As of #70 those are part record IDs:
/// every `KeyRotation` record is a part, so pass that intersection straight
/// through. Parts that `ChannelKeys` evicted under a flood (their IDs come
/// back from `apply`) are fetched again. A part we re-send is the same
/// stored record, never re-signed.
/// Messages with `Verdict::Pending` (auth_deps not here yet) are neither
/// shown nor forwarded until they resolve (B4).
/// API as of #51: build one `Arc<MemberSecret>` (`MemberSecret::new(
/// identity.signing_key())`) at unlock and share it across every
/// `ChannelKeys::new(server, channel, secret)`. `apply()` returns
/// `(Applied, Vec<RecordId>)`: hand the evicted IDs to sync to re-request.
/// GAP-8: a rotation that authority accepts after `ChannelKeys` evicted it
/// is fetched again (the evicted list can hold the rotation just applied),
/// and `ChannelKeys` exist only for channels authority knows.
pub trait GroupCipher: Send + Sync {
    fn membership_changed(&self, server: ServerId, members: &BTreeSet<PublicKey>) -> CipherOutcome;
}

/// Stand-in until groupkeys is merged: never claims a renewal it didn't do.
pub struct PendingGroupKeys;

impl GroupCipher for PendingGroupKeys {
    fn membership_changed(&self, _: ServerId, _: &BTreeSet<PublicKey>) -> CipherOutcome {
        CipherOutcome::Unchanged
    }
}

/// A shared [`SecretStore`], so the shell can hand in `OsKeychain` and tests a
/// `MemoryStore` they still inspect.
#[derive(Clone)]
pub struct SharedSecrets(pub Arc<dyn SecretStore + Send + Sync>);

impl SecretStore for SharedSecrets {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        self.0.load(name)
    }

    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError> {
        self.0.save(name, secret)
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.delete(name)
    }
}

impl std::fmt::Debug for SharedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedSecrets(..)")
    }
}

/// Everything a [`crate::Core`] is plugged into.
pub struct Ports {
    pub secrets: Arc<dyn SecretStore + Send + Sync>,
    pub network: Arc<dyn Network>,
    pub events: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn GroupCipher>,
}
