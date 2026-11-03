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
///
/// Startup (#64): sync refuses to start when it can't lock down `node.key`
/// (the Windows ACL). Surface that as a clear error the UI shows, an error
/// code or a `notice`, never a silently dead sync.
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
///
/// How service.rs uses it:
/// - send: `encrypt(server, channel, plaintext)` and put the result in the
///   `Message` record body; on `RotationRequired`/`NoKey` tell the UI the
///   channel can't be written to yet;
/// - receive: for an accepted `Message` record, `decrypt(server, channel,
///   author, body)`; show `Opened { accepted: false }` as untrusted (#21);
/// - every `KeyRotation` record (accepted or not): `apply_rotation(record)`,
///   then re-request the evicted record IDs it returns;
/// - whenever authority's accepted set changes: `set_accepted(server, ids)`
///   with that server's accepted `KeyRotation` record IDs.
///
/// The caller MUST:
/// - call `encrypt`, `decrypt`, `apply_rotation` and `rotate` only for
///   channels authority knows. As a backstop the cipher holds keys for at
///   most `MAX_CHANNELS_PER_SERVER` (256) channels per server and refuses
///   new ones past that;
/// - after every `unlock`, call `set_accepted` and then replay every stored
///   `KeyRotation` record through `apply_rotation`. `lock` drops all keys.
pub trait GroupCipher: Send + Sync {
    fn membership_changed(&self, server: ServerId, members: &BTreeSet<PublicKey>) -> CipherOutcome;

    /// Seals a channel message from us under the channel's current key.
    fn encrypt(
        &self,
        server: ServerId,
        channel: records::ChannelId,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let _ = (server, channel, plaintext);
        Err(CipherError::Unavailable)
    }

    /// Opens a channel message body whose record `author` signed.
    fn decrypt(
        &self,
        server: ServerId,
        channel: records::ChannelId,
        author: PublicKey,
        body: &[u8],
    ) -> Result<Opened, CipherError> {
        let _ = (server, channel, author, body);
        Err(CipherError::Unavailable)
    }

    /// Takes in one `KeyRotation` record. Returns the part records evicted
    /// to stay within the probation caps, for sync to fetch again.
    fn apply_rotation(&self, record: &Record) -> Result<Vec<records::RecordId>, CipherError> {
        let _ = record;
        Err(CipherError::Unavailable)
    }

    /// The `KeyRotation` records of `server` that authority accepts now
    /// (deny by default: only these can become a channel's key).
    fn set_accepted(&self, server: ServerId, accepted: &BTreeSet<records::RecordId>) {
        let _ = (server, accepted);
    }

    /// At identity unlock: `MemberSecret::new(signing_key)`. Until then
    /// everything but bookkeeping is `Unavailable`.
    fn unlock(&self, secret: Arc<groupkeys::MemberSecret>) {
        let _ = secret;
    }

    /// At app lock and sign-out: drops the secret and every channel's keys.
    fn lock(&self) {}

    /// The unsigned `KeyRotation` part bodies (at most 32) of a fresh key for
    /// `members`, built on the channel's current key. Service signs every
    /// part with our identity and publishes **all of them or none**, and never
    /// re-signs a part (a second record for one part gets the rotation
    /// refused); to retry, it resends the same records.
    fn rotate(
        &self,
        server: ServerId,
        channel: records::ChannelId,
        members: &BTreeSet<PublicKey>,
    ) -> Result<Vec<Vec<u8>>, CipherError> {
        let _ = (server, channel, members);
        Err(CipherError::Unavailable)
    }
}

/// A decrypted channel message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub plaintext: Vec<u8>,
    /// False: sealed under a rotation authority doesn't accept; show it as
    /// untrusted, never as a normal message.
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CipherError {
    /// No group encryption in this build.
    Unavailable,
    /// We hold no key for the channel, or for the message's epoch.
    NoKey,
    /// Removed members still hold the key; a moderator must rotate first.
    RotationRequired,
    /// Malformed or tampered input, or the author didn't hold the key.
    Rejected,
}

/// Stand-in until groupkeys is wired in: never claims a renewal it didn't do,
/// and refuses to encrypt or decrypt.
pub struct PendingGroupKeys;

impl GroupCipher for PendingGroupKeys {
    fn membership_changed(&self, _: ServerId, _: &BTreeSet<PublicKey>) -> CipherOutcome {
        CipherOutcome::Unchanged
    }

    fn encrypt(
        &self,
        _: ServerId,
        _: records::ChannelId,
        _: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        Err(CipherError::Unavailable)
    }

    fn decrypt(
        &self,
        _: ServerId,
        _: records::ChannelId,
        _: PublicKey,
        _: &[u8],
    ) -> Result<Opened, CipherError> {
        Err(CipherError::Unavailable)
    }

    fn apply_rotation(&self, _: &Record) -> Result<Vec<records::RecordId>, CipherError> {
        Err(CipherError::Unavailable)
    }

    fn set_accepted(&self, _: ServerId, _: &BTreeSet<records::RecordId>) {}
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
