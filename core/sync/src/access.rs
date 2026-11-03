//! Members-only sync (GAP-4, design in `GAP-4.md`).
//!
//! - Every server has a random [`ServerSecret`], carried in invite links and
//!   never put into a record. On the wire a log is named only by its
//!   [`Handle`], derived from the secret, so knowing the server ID is not
//!   enough to ask for a log or to recognise its gossip topic.
//! - Gossip is sealed (XChaCha20-Poly1305, random 192-bit nonce) under a key
//!   per log, derived from the secret. AD = version ‖ handle. Sealing never
//!   replaces the record checks: an opened record still goes through decode,
//!   the derived log and the store.
//! - A pull carries a [`Binding`]: the identity key's signature over the
//!   requester's peer ID, the server's control handle and an expiry. The
//!   answerer checks it against the Noise-authenticated remote peer ID.
//! - Channel logs are served only to identities the [`ReadGate`] accepts.
//!   The control log is served to any bound requester that knows the secret
//!   (a joiner needs it before its own `Join` exists).
//!
//! The secret is a bearer credential: it is never logged (its `Debug` is
//! redacted) and never goes into a URL sent anywhere. Handles never go into
//! the DHT.

use std::fmt;
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use libp2p::PeerId;
use rand_core::{OsRng, RngCore};
use records::{PublicKey, Scope, SigningKey};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ControlLog;

pub const SECRET_LEN: usize = 32;

const TOPIC_CONTEXT: &str = "mvs/topic/v1";
const GOSSIP_CONTEXT: &str = "mvs/gossip/v1";
const BINDING_CONTEXT: &[u8] = b"mvs/peer-binding/v1";

const SEAL_VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
/// Bytes sealing adds to a gossip message.
pub(crate) const SEAL_OVERHEAD: usize = 1 + NONCE_LEN + TAG_LEN;

/// Longest validity of a binding, and the clock skew allowed either way.
pub const MAX_BINDING_TTL: Duration = Duration::from_secs(24 * 3600);
pub const CLOCK_SKEW: Duration = Duration::from_secs(600);

/// A server's secret: whoever holds it can name the server's logs, read its
/// gossip and pull its control log. Zeroed on drop, never printed.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct ServerSecret([u8; SECRET_LEN]);

impl ServerSecret {
    pub fn from_bytes(bytes: [u8; SECRET_LEN]) -> ServerSecret {
        ServerSecret(bytes)
    }

    /// A fresh random secret, for a new server.
    pub fn generate() -> ServerSecret {
        let mut bytes = [0; SECRET_LEN];
        OsRng.fill_bytes(&mut bytes);
        ServerSecret(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }

    /// The name of one of the server's logs on the wire.
    pub fn handle(&self, scope: Scope) -> Handle {
        Handle(self.derive(TOPIC_CONTEXT, scope))
    }

    /// The key that seals one log's gossip.
    pub(crate) fn gossip_key(&self, scope: Scope) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.derive(GOSSIP_CONTEXT, scope))
    }

    /// `BLAKE3::derive_key(context, secret ‖ scope)`; scope is `0x00` for the
    /// control log, `0x01 ‖ channel id` for a channel.
    fn derive(&self, context: &str, scope: Scope) -> [u8; 32] {
        let mut material = Zeroizing::new(Vec::with_capacity(SECRET_LEN + 33));
        material.extend_from_slice(&self.0);
        match scope {
            Scope::Control => material.push(0),
            Scope::Channel(channel) => {
                material.push(1);
                material.extend_from_slice(channel.as_bytes());
            }
        }
        blake3::derive_key(context, &material)
    }
}

impl fmt::Debug for ServerSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerSecret(..)")
    }
}

/// A log's name on the wire: its gossip topic and the `log` of a pull.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle([u8; 32]);

impl Handle {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// `h/<64 lowercase hex>`.
    pub(crate) fn wire(&self) -> String {
        let mut s = String::with_capacity(66);
        s.push_str("h/");
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub(crate) fn parse(wire: &str) -> Option<Handle> {
        let hex = wire.strip_prefix("h/")?.as_bytes();
        if hex.len() != 64 {
            return None;
        }
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0; 32];
        for (i, pair) in hex.chunks(2).enumerate() {
            out[i] = nibble(pair[0])? << 4 | nibble(pair[1])?;
        }
        Some(Handle(out))
    }
}

/// Seals a record for a log's gossip topic: `version ‖ nonce ‖ ciphertext`.
pub(crate) fn seal(key: &[u8; 32], handle: &Handle, record: &[u8]) -> Vec<u8> {
    let mut nonce = [0; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ad = associated_data(handle);
    let sealed = XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: record, aad: &ad })
        .expect("in-memory encryption does not fail");
    let mut out = Vec::with_capacity(SEAL_OVERHEAD + record.len());
    out.push(SEAL_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// Opens a sealed gossip message for the log `handle` names; None if it
/// isn't one (wrong version, key or topic, or tampered).
pub(crate) fn open(key: &[u8; 32], handle: &Handle, sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < SEAL_OVERHEAD || sealed[0] != SEAL_VERSION {
        return None;
    }
    let (nonce, ciphertext) = sealed[1..].split_at(NONCE_LEN);
    let ad = associated_data(handle);
    XChaCha20Poly1305::new(key.into()).decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad: &ad }).ok()
}

fn associated_data(handle: &Handle) -> [u8; 33] {
    let mut ad = [0; 33];
    ad[0] = SEAL_VERSION;
    ad[1..].copy_from_slice(&handle.0);
    ad
}

/// The bytes a [`Binder`] signs: `mvs/peer-binding/v1 ‖ len(peer) ‖ peer ID
/// ‖ control handle ‖ expiry` (expiry in Unix seconds, big-endian).
pub fn binding_message(peer: &PeerId, control: &Handle, expiry: u64) -> Vec<u8> {
    let peer = peer.to_bytes();
    let mut message = Vec::with_capacity(BINDING_CONTEXT.len() + 1 + peer.len() + 32 + 8);
    message.extend_from_slice(BINDING_CONTEXT);
    message.push(peer.len() as u8);
    message.extend_from_slice(&peer);
    message.extend_from_slice(&control.0);
    message.extend_from_slice(&expiry.to_be_bytes());
    message
}

/// Signs peer bindings with the user's identity key. Implemented by the
/// integration layer over `core/identity`.
pub trait Binder: Send + Sync + 'static {
    fn identity(&self) -> PublicKey;
    /// Signs `message`, which is always a [`binding_message`]. An
    /// implementation should refuse to sign anything that doesn't start with
    /// `mvs/peer-binding/v1`.
    fn sign_binding(&self, message: &[u8]) -> [u8; 64];
}

/// A plain Ed25519 key as a binder (tests, the harness).
impl Binder for SigningKey {
    fn identity(&self) -> PublicKey {
        PublicKey::from_bytes(self.verifying_key().to_bytes())
    }

    fn sign_binding(&self, message: &[u8]) -> [u8; 64] {
        assert!(message.starts_with(BINDING_CONTEXT), "not a peer binding");
        ed25519_dalek::Signer::sign(self, message).to_bytes()
    }
}

/// May a bound identity pull a server's channel logs? Implemented by the
/// integration layer over the current authority state: a member, and not
/// kicked. Unlike the storage gate this need not be monotone (serving is
/// not storing). Called on a blocking thread with a read-only store.
pub trait ReadGate: Send + Sync + 'static {
    fn may_read(&self, reader: &PublicKey, control: &ControlLog<'_>) -> bool;
}

/// Lets every bound identity read. For the harness and tests only.
impl ReadGate for crate::AcceptAll {
    fn may_read(&self, _: &PublicKey, _: &ControlLog<'_>) -> bool {
        true
    }
}

/// A signed peer binding, as carried in a pull.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    identity: ByteBuf,
    expiry: u64,
    signature: ByteBuf,
}

impl Binding {
    pub(crate) fn sign(binder: &dyn Binder, peer: &PeerId, control: &Handle, expiry: u64) -> Binding {
        let signature = binder.sign_binding(&binding_message(peer, control, expiry));
        Binding {
            identity: ByteBuf::from(binder.identity().as_bytes().to_vec()),
            expiry,
            signature: ByteBuf::from(signature.to_vec()),
        }
    }

    pub(crate) fn expiry(&self) -> u64 {
        self.expiry
    }

    /// The identity this binding proves for `peer` (the connection's
    /// Noise-authenticated remote) on the server whose control handle is
    /// `control`, at Unix time `now`. None if it doesn't verify, has
    /// expired, or claims more than [`MAX_BINDING_TTL`] ahead.
    pub(crate) fn verify(&self, peer: &PeerId, control: &Handle, now: u64) -> Option<PublicKey> {
        let skew = CLOCK_SKEW.as_secs();
        if self.expiry.saturating_add(skew) < now || self.expiry > now + MAX_BINDING_TTL.as_secs() + skew {
            return None;
        }
        let identity: [u8; 32] = self.identity.as_slice().try_into().ok()?;
        let signature: [u8; 64] = self.signature.as_slice().try_into().ok()?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&identity).ok()?;
        key.verify_strict(&binding_message(peer, control, self.expiry), &ed25519_dalek::Signature::from_bytes(&signature))
            .ok()?;
        Some(PublicKey::from_bytes(identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{channel, test_key};

    fn secret() -> ServerSecret {
        ServerSecret::from_bytes([9; SECRET_LEN])
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn handles_and_keys_are_pinned_and_domain_separated() {
        let control = secret().handle(Scope::Control);
        let chan = secret().handle(Scope::Channel(channel()));
        // Pinned: changing the derivation silently would split the network.
        assert_eq!(hex(control.as_bytes()), PINNED_CONTROL_HANDLE);
        assert_eq!(control, Handle(blake3::derive_key("mvs/topic/v1", &[[9; 32].as_slice(), &[0]].concat())));
        let mut chan_material = vec![9; 32];
        chan_material.push(1);
        chan_material.extend_from_slice(channel().as_bytes());
        assert_eq!(chan, Handle(blake3::derive_key("mvs/topic/v1", &chan_material)));
        // Handle, gossip key and another secret's handle all differ.
        assert_ne!(control, chan);
        assert_ne!(control.0, *secret().gossip_key(Scope::Control));
        assert_ne!(control, ServerSecret::from_bytes([8; 32]).handle(Scope::Control));
        assert_eq!(Handle::parse(&control.wire()), Some(control));
        assert_eq!(Handle::parse("h/00"), None);
        assert_eq!(Handle::parse(&control.wire().to_uppercase()), None);
        assert_eq!(format!("{:?}", secret()), "ServerSecret(..)");
    }

    const PINNED_CONTROL_HANDLE: &str = "31cfc551a77fe40fd82ca1175991deb9221386644f74c8e3870de58e84a08b81";

    #[test]
    fn sealed_gossip_opens_only_with_its_key_and_topic() {
        let s = secret();
        let (control, chan) = (s.handle(Scope::Control), s.handle(Scope::Channel(channel())));
        let key = s.gossip_key(Scope::Control);
        let sealed = seal(&key, &control, b"record");
        assert_eq!(sealed.len(), SEAL_OVERHEAD + 6);
        assert_eq!(open(&key, &control, &sealed).as_deref(), Some(&b"record"[..]));
        // Random nonces: sealing twice gives different bytes.
        assert_ne!(seal(&key, &control, b"record"), sealed);
        // Moved to another topic, another key, tampered or truncated: refused.
        assert_eq!(open(&key, &chan, &sealed), None);
        assert_eq!(open(&s.gossip_key(Scope::Channel(channel())), &control, &sealed), None);
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(open(&key, &control, &tampered), None);
        let mut version = sealed.clone();
        version[0] = 2;
        assert_eq!(open(&key, &control, &version), None);
        assert_eq!(open(&key, &control, &sealed[..SEAL_OVERHEAD - 1]), None);
    }

    #[test]
    fn bindings_hold_only_for_their_peer_server_and_time() {
        let key = test_key(1);
        let (peer, other_peer) = (PeerId::random(), PeerId::random());
        let control = secret().handle(Scope::Control);
        let other_server = ServerSecret::from_bytes([8; 32]).handle(Scope::Control);
        let now = 1_800_000_000;
        let binding = Binding::sign(&key, &peer, &control, now + 3600);
        assert_eq!(binding.verify(&peer, &control, now), Some(key.identity()));
        // Shown by another peer, or replayed into another server: refused.
        assert_eq!(binding.verify(&other_peer, &control, now), None);
        assert_eq!(binding.verify(&peer, &other_server, now), None);
        // Expired (beyond the skew), or claiming more than 24 h ahead: refused.
        assert_eq!(binding.verify(&peer, &control, now + 3600 + 601), None);
        assert!(binding.verify(&peer, &control, now + 3600 + 599).is_some());
        let too_long = Binding::sign(&key, &peer, &control, now + 25 * 3600);
        assert_eq!(too_long.verify(&peer, &control, now), None);
        // A changed expiry or identity breaks the signature.
        let mut forged = binding.clone();
        forged.expiry += 1;
        assert_eq!(forged.verify(&peer, &control, now), None);
        let mut swapped = binding.clone();
        swapped.identity = ByteBuf::from(test_key(2).identity().as_bytes().to_vec());
        assert_eq!(swapped.verify(&peer, &control, now), None);
    }

    #[test]
    #[should_panic(expected = "not a peer binding")]
    fn the_key_binder_signs_only_bindings() {
        test_key(1).sign_binding(b"something else");
    }
}
