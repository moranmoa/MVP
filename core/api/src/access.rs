//! Members-only sync (GAP-4): what the core gives `sync`'s access layer.
//!
//! - [`KeyBinder`] signs our peer bindings with the identity key. It signs
//!   only `mvs/peer-binding/v1` messages and panics on anything else, so the
//!   identity key can never be made to sign some other message through it.
//!   While locked it signs nothing: the zero signature no peer accepts.
//! - [`MemberReadGate`] serves a server's channel logs only to its members
//!   who aren't kicked, judged on the current resolved control state (not a
//!   monotone gate: a kick takes reading away).
//!
//! The server secret itself is kept by the core, sealed in its database
//! (`db::save_server_secret`), and never logged.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, RwLock};

use authority::{ChannelGraph, Entry, Resolver, Verdict};
use ed25519_dalek::Signer;
use records::{PublicKey, Record, RecordId, ServerId, SigningKey, Stub};
use sync::{Binder, ChannelLog, ControlLog, Held, ReadGate, StubGate, StubVerdict};

/// The only messages [`KeyBinder`] signs start with this.
pub const BINDING_PREFIX: &[u8] = b"mvs/peer-binding/v1";

/// The identity key, from unlock to lock, for peer bindings only.
pub struct KeyBinder {
    identity: PublicKey,
    key: RwLock<Option<SigningKey>>,
}

impl KeyBinder {
    /// Locked: signs nothing until [`Self::unlock`]. `identity` is what
    /// peers are told before then (nobody, until an identity exists).
    pub fn locked() -> KeyBinder {
        KeyBinder { identity: PublicKey::from_bytes([0; 32]), key: RwLock::new(None) }
    }

    pub fn unlock(&self, key: &SigningKey) {
        *self.key.write().expect("not poisoned") = Some(key.clone());
    }

    /// Drops the key (zeroized on drop).
    pub fn lock(&self) {
        *self.key.write().expect("not poisoned") = None;
    }
}

impl Binder for KeyBinder {
    fn identity(&self) -> PublicKey {
        match self.key.read().expect("not poisoned").as_ref() {
            Some(key) => PublicKey::from_bytes(key.verifying_key().to_bytes()),
            None => self.identity,
        }
    }

    fn sign_binding(&self, message: &[u8]) -> [u8; 64] {
        // A hard refusal: anything else would let the node make the
        // identity key sign arbitrary bytes.
        assert!(
            message.starts_with(BINDING_PREFIX),
            "KeyBinder signs only mvs/peer-binding/v1 messages"
        );
        match self.key.read().expect("not poisoned").as_ref() {
            Some(key) => key.sign(message).to_bytes(),
            None => [0; 64],
        }
    }
}

/// Channel logs for members only, from the current control state. The
/// verdict is cached per server and reader together with the control log's
/// record count, and decided again whenever that count changes (a join, a
/// kick).
#[derive(Default)]
pub struct MemberReadGate {
    cache: Mutex<HashMap<(ServerId, PublicKey), (u64, bool)>>,
}

/// Readers remembered across all servers; past this the cache is cleared.
const MAX_CACHED: usize = 4096;

impl MemberReadGate {
    pub fn new() -> MemberReadGate {
        MemberReadGate::default()
    }
}

impl ReadGate for MemberReadGate {
    fn may_read(&self, reader: &PublicKey, control: &ControlLog<'_>) -> bool {
        let Ok(count) = control.count() else { return false };
        let Ok(records) = control.records() else { return false };
        let Some(server) = records.first().map(records::Record::server) else { return false };
        let key = (server, *reader);
        if let Some((seen, verdict)) = self.cache.lock().expect("not poisoned").get(&key) {
            if *seen == count {
                return *verdict;
            }
        }
        let state = authority::resolve(server, &records);
        let verdict = state.is_member(*reader) && !state.kicked.contains(reader);
        let mut cache = self.cache.lock().expect("not poisoned");
        if cache.len() >= MAX_CACHED {
            cache.clear();
        }
        cache.insert(key, (count, verdict));
        verdict
    }
}

/// B6-p: sync stores a peer's stub in place of a record, or prunes one of
/// ours, only once authority accepts the stub, that is, a held accepted
/// delete or newer edit replaced its record. A hostile peer serving stubs
/// can't make messages disappear.
pub struct MemberStubGate;

/// Sync's view of one channel, as `authority` asks about it.
struct SyncGraph<'a, 'b>(&'a ChannelLog<'b>);

impl SyncGraph<'_, '_> {
    fn held(&self, id: RecordId) -> Option<Held> {
        self.0.entry(&id).ok().flatten()
    }
}

impl ChannelGraph for SyncGraph<'_, '_> {
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![of];
        while let Some(id) = stack.pop() {
            if id == ancestor {
                return true;
            }
            if seen.insert(id) {
                if let Some(held) = self.held(id) {
                    stack.extend(held_entry(held).deps().iter().copied());
                }
            }
        }
        false
    }

    fn record(&self, id: RecordId) -> Option<Record> {
        match self.held(id)? {
            Held::Full(record) => Some(record),
            Held::Stub(_) => None,
        }
    }

    fn entry(&self, id: RecordId) -> Option<Entry> {
        self.held(id).map(held_entry)
    }

    fn children(&self, id: RecordId) -> Vec<RecordId> {
        self.0.children(&id).map(|c| c.iter().map(Held::id).collect()).unwrap_or_default()
    }
}

fn held_entry(held: Held) -> Entry {
    match held {
        Held::Full(record) => Entry::Full(record),
        Held::Stub(stub) => Entry::Pruned(stub),
    }
}

impl StubGate for MemberStubGate {
    fn check(&self, stub: &Stub, channel: &ChannelLog<'_>) -> StubVerdict {
        let Ok(control) = channel.control().records() else { return StubVerdict::Pending };
        let mut resolver = Resolver::new(stub.server());
        for record in control {
            resolver.insert(record);
        }
        match authority::check_stub(&resolver, stub, &SyncGraph(channel)) {
            // Replaced by a held, accepted delete or edit (a voided record
            // may go too: it is never shown).
            Verdict::Accept | Verdict::Voided { .. } | Verdict::Hidden { .. } => {
                StubVerdict::Accept
            }
            Verdict::Pending { .. } => StubVerdict::Pending,
            Verdict::Refused(_) => StubVerdict::Reject,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use authority::Action;
    use sync::{AcceptAll, Store};

    #[test]
    #[should_panic(expected = "only mvs/peer-binding/v1")]
    fn the_binder_refuses_other_messages() {
        let binder = KeyBinder::locked();
        binder.unlock(&SigningKey::from_bytes(&[3; 32]));
        let _ = binder.sign_binding(b"mvs/something-else/v1 hello");
    }

    #[test]
    fn the_binder_signs_bindings_only_while_unlocked() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let binder = KeyBinder::locked();
        let message = [BINDING_PREFIX, b"peer, handle, expiry"].concat();
        assert_eq!(binder.sign_binding(&message), [0; 64]);
        binder.unlock(&key);
        assert_eq!(binder.identity().as_bytes(), key.verifying_key().as_bytes());
        let signature = ed25519_dalek::Signature::from_bytes(&binder.sign_binding(&message));
        assert!(key.verifying_key().verify_strict(&message, &signature).is_ok());
        binder.lock();
        assert_eq!(binder.sign_binding(&message), [0; 64]);
    }

    #[test]
    fn members_read_until_kicked() {
        let owner = SigningKey::from_bytes(&[1; 32]);
        let member = SigningKey::from_bytes(&[2; 32]);
        let stranger = SigningKey::from_bytes(&[3; 32]);
        let public = |k: &SigningKey| PublicKey::from_bytes(k.verifying_key().to_bytes());
        let genesis = authority::sign_genesis(&owner, [0; 16], 1, b"{}").unwrap();
        let server = genesis.server();
        let invite = authority::sign_control(
            &owner,
            server,
            1,
            vec![genesis.id()],
            1,
            &Action::Invite { expires_ms: 0, max_uses: 0 },
        )
        .unwrap();
        let join = authority::sign_control(
            &member,
            server,
            0,
            vec![invite.id()],
            2,
            &Action::Join { invite: invite.id() },
        )
        .unwrap();
        let mut store = Store::in_memory().unwrap();
        for record in [&genesis, &invite, &join] {
            store.append(record, &AcceptAll).unwrap();
        }
        let gate = MemberReadGate::new();
        let may_read = |store: &Store, who: &SigningKey| {
            gate.may_read(&public(who), &store.control_log(server))
        };
        assert!(may_read(&store, &owner));
        assert!(may_read(&store, &member));
        assert!(!may_read(&store, &stranger));

        let kick = authority::sign_control(
            &owner,
            server,
            2,
            vec![join.id()],
            3,
            &Action::kick(public(&member)),
        )
        .unwrap();
        store.append(&kick, &AcceptAll).unwrap();
        assert!(!may_read(&store, &member), "a kick takes reading away");
        assert!(may_read(&store, &owner));
    }
}
