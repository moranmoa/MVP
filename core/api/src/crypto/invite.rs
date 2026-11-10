//! Invite envelopes and their signed publications (T8-02 piece 2,
//! `docs/design/short-invite-link.md` §3–§5). Pure: no network.
//!
//! From a v1 link's invite key k (`crate::invite_link`):
//! - `h = derive_key("mvs/invite-handle/v1", k)`: the envelope's name on the wire;
//! - `e = derive_key("mvs/invite-seal/v1", k)`: the envelope's AEAD key.
//!
//! The envelope is XChaCha20-Poly1305 under `e` (the crate `sync` already
//! uses), AAD `0x01 ‖ server ID prefix`, plaintext
//! `server ID ‖ invite record ID ‖ server secret`: 24-byte nonce + 96 + 16 =
//! 136 bytes. Its [`InvitePublication`] is signed by the invite's author over
//! `"mvs/invite-envelope/v1\0" ‖ invite ‖ h ‖ envelope` (C1), so members
//! only serve envelopes an inviter really published.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use records::{PublicKey, RecordId, ServerId};
use zeroize::Zeroizing;

use crate::invite_link::{KEY_LEN, SERVER_PREFIX_LEN, V1};

const HANDLE_CONTEXT: &str = "mvs/invite-handle/v1";
const SEAL_CONTEXT: &str = "mvs/invite-seal/v1";
const SIGN_DOMAIN: &[u8] = b"mvs/invite-envelope/v1\0";

const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const PLAIN_LEN: usize = 32 + 32 + 32;
/// A v1 envelope: nonce, sealed plaintext, tag.
pub const ENVELOPE_LEN: usize = NONCE_LEN + PLAIN_LEN + TAG_LEN;
/// Largest envelope a publication may carry (C2); later versions may grow.
pub const MAX_ENVELOPE: usize = 256;
const SIG_LEN: usize = 64;
/// Largest encoded publication: what a `/mvs/invite/1` answer carries.
pub const MAX_PUBLICATION: usize = 32 + 32 + 2 + MAX_ENVELOPE + SIG_LEN;

/// The envelope's name on the wire.
pub fn handle(key: &[u8; KEY_LEN]) -> [u8; 32] {
    blake3::derive_key(HANDLE_CONTEXT, key)
}

fn seal_key(key: &[u8; KEY_LEN]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(SEAL_CONTEXT, key))
}

fn aad(server_prefix: &[u8; SERVER_PREFIX_LEN]) -> [u8; 1 + SERVER_PREFIX_LEN] {
    let mut aad = [0u8; 1 + SERVER_PREFIX_LEN];
    aad[0] = V1;
    aad[1..].copy_from_slice(server_prefix);
    aad
}

fn prefix_of(server: &ServerId) -> [u8; SERVER_PREFIX_LEN] {
    let mut prefix = [0u8; SERVER_PREFIX_LEN];
    prefix.copy_from_slice(&server.as_bytes()[..SERVER_PREFIX_LEN]);
    prefix
}

/// What an envelope holds.
pub struct Opened {
    pub server: ServerId,
    pub invite: RecordId,
    pub secret: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Opened(..)")
    }
}

/// Seals `server ‖ invite ‖ secret` under k, with a fresh random nonce.
pub fn seal(
    key: &[u8; KEY_LEN],
    server: &ServerId,
    invite: &RecordId,
    secret: &[u8; 32],
) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    seal_with_nonce(key, server, invite, secret, &nonce)
}

fn seal_with_nonce(
    key: &[u8; KEY_LEN],
    server: &ServerId,
    invite: &RecordId,
    secret: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let mut plain = Zeroizing::new([0u8; PLAIN_LEN]);
    plain[..32].copy_from_slice(server.as_bytes());
    plain[32..64].copy_from_slice(invite.as_bytes());
    plain[64..].copy_from_slice(secret);
    let aad = aad(&prefix_of(server));
    let sealed = XChaCha20Poly1305::new(seal_key(key).as_ref().into())
        .encrypt(XNonce::from_slice(nonce), Payload { msg: &plain[..], aad: &aad })
        .expect("XChaCha20-Poly1305 seals any 96-byte message");
    let mut envelope = Vec::with_capacity(ENVELOPE_LEN);
    envelope.extend_from_slice(nonce);
    envelope.extend_from_slice(&sealed);
    envelope
}

/// Opens an envelope under k. `None` unless it is a v1 envelope sealed under
/// this k for a server whose ID starts with the link's prefix: answers are
/// untrusted (C3), so any failure just means "try the next one".
pub fn open(
    key: &[u8; KEY_LEN],
    server_prefix: &[u8; SERVER_PREFIX_LEN],
    envelope: &[u8],
) -> Option<Opened> {
    if envelope.len() != ENVELOPE_LEN {
        return None;
    }
    let (nonce, sealed) = envelope.split_at(NONCE_LEN);
    let aad = aad(server_prefix);
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(seal_key(key).as_ref().into())
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &aad })
            .ok()?,
    );
    let server = ServerId::from_bytes(plain[..32].try_into().ok()?);
    if prefix_of(&server) != *server_prefix {
        return None;
    }
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&plain[64..]);
    Some(Opened { server, invite: RecordId::from_bytes(plain[32..64].try_into().ok()?), secret })
}

/// An envelope as the inviter publishes it to members (design §4).
#[derive(Clone, PartialEq, Eq)]
pub struct InvitePublication {
    pub invite: RecordId,
    pub handle: [u8; 32],
    pub envelope: Vec<u8>,
    sig: [u8; SIG_LEN],
}

impl std::fmt::Debug for InvitePublication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The handle names a live invite (GAP-4b residual): not for logs either.
        f.write_str("InvitePublication(..)")
    }
}

fn signed_message(invite: &RecordId, handle: &[u8; 32], envelope: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGN_DOMAIN.len() + 64 + envelope.len());
    message.extend_from_slice(SIGN_DOMAIN);
    message.extend_from_slice(invite.as_bytes());
    message.extend_from_slice(handle);
    message.extend_from_slice(envelope);
    message
}

impl InvitePublication {
    /// Signed by the invite's author. `None` for an envelope over the cap.
    pub fn sign(
        author: &SigningKey,
        invite: RecordId,
        handle: [u8; 32],
        envelope: Vec<u8>,
    ) -> Option<Self> {
        if envelope.is_empty() || envelope.len() > MAX_ENVELOPE {
            return None;
        }
        let sig = author.sign(&signed_message(&invite, &handle, &envelope)).to_bytes();
        Some(InvitePublication { invite, handle, envelope, sig })
    }

    /// Whether `author` (the named invite's author in the control log)
    /// signed it. Liveness of the invite is the caller's check (C1).
    pub fn verify(&self, author: &PublicKey) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(author.as_bytes()) else { return false };
        let message = signed_message(&self.invite, &self.handle, &self.envelope);
        key.verify_strict(&message, &Signature::from_bytes(&self.sig)).is_ok()
    }

    /// Canonical: `invite (32) ‖ handle (32) ‖ envelope length (u16 BE) ‖
    /// envelope ‖ signature (64)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 32 + 2 + self.envelope.len() + SIG_LEN);
        out.extend_from_slice(self.invite.as_bytes());
        out.extend_from_slice(&self.handle);
        out.extend_from_slice(&(self.envelope.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.envelope);
        out.extend_from_slice(&self.sig);
        out
    }

    /// The inverse of [`encode`](Self::encode); `None` for anything else
    /// (a length that doesn't add up, an empty or oversize envelope,
    /// trailing bytes). The signature is not checked here.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_PUBLICATION {
            return None;
        }
        let (invite, rest) = bytes.split_at_checked(32)?;
        let (handle, rest) = rest.split_at_checked(32)?;
        let (len, rest) = rest.split_at_checked(2)?;
        let len = u16::from_be_bytes(len.try_into().ok()?) as usize;
        if len == 0 || len > MAX_ENVELOPE {
            return None;
        }
        let (envelope, sig) = rest.split_at_checked(len)?;
        Some(InvitePublication {
            invite: RecordId::from_bytes(invite.try_into().ok()?),
            handle: handle.try_into().ok()?,
            envelope: envelope.to_vec(),
            sig: sig.try_into().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; KEY_LEN] = [0x11; KEY_LEN];
    const NONCE: [u8; NONCE_LEN] = [0x22; NONCE_LEN];
    fn server() -> ServerId {
        ServerId::from_bytes(std::array::from_fn(|i| i as u8 + 0x40))
    }
    fn invite() -> RecordId {
        RecordId::from_bytes([0x33; 32])
    }
    const SECRET: [u8; 32] = [0x44; 32];
    fn author() -> SigningKey {
        SigningKey::from_bytes(&[0x55; 32])
    }
    fn publication() -> InvitePublication {
        let envelope = seal_with_nonce(&KEY, &server(), &invite(), &SECRET, &NONCE);
        InvitePublication::sign(&author(), invite(), handle(&KEY), envelope).unwrap()
    }

    // Pinned: changing a label, the layout or the signed bytes silently
    // would orphan every published invite.
    const PINNED_HANDLE: &str = "dc75fc1ed12aa1b8ef5a58901c53b78762c36d67385eaa82c2549a3acd9c69c4";
    const PINNED_SEAL_KEY: &str =
        "97607171cc35c01b94a55dd20671ce0a8a651ef0da58e88f292c69edec352b55";
    /// BLAKE3 of the encoded publication (fixed k, nonce, IDs, secret, author).
    const PINNED_PUBLICATION: &str =
        "dc5788033102b66a30651d7105e32651a45f6bfedb1ed9b3fc080c0c3c82d3d3";

    #[test]
    fn keys_and_publication_are_pinned_and_domain_separated() {
        assert_eq!(hex::encode(handle(&KEY)), PINNED_HANDLE);
        assert_eq!(hex::encode(*seal_key(&KEY)), PINNED_SEAL_KEY);
        assert_eq!(handle(&KEY), blake3::derive_key("mvs/invite-handle/v1", &KEY));
        assert_eq!(*seal_key(&KEY), blake3::derive_key("mvs/invite-seal/v1", &KEY));
        assert_ne!(handle(&KEY), *seal_key(&KEY));
        // Ed25519 signatures are deterministic: the whole publication is pinned.
        assert_eq!(blake3::hash(&publication().encode()).to_hex().as_str(), PINNED_PUBLICATION);
    }

    #[test]
    fn an_envelope_opens_only_with_its_key_and_prefix() {
        let envelope = seal(&KEY, &server(), &invite(), &SECRET);
        assert_eq!(envelope.len(), ENVELOPE_LEN);
        let prefix = prefix_of(&server());
        let opened = open(&KEY, &prefix, &envelope).unwrap();
        assert_eq!((opened.server, opened.invite, *opened.secret), (server(), invite(), SECRET));
        assert_eq!(format!("{opened:?}"), "Opened(..)");
        // Another k, another prefix, a flipped byte anywhere, a wrong length.
        assert!(open(&[0x12; KEY_LEN], &prefix, &envelope).is_none());
        assert!(open(&KEY, &[0; SERVER_PREFIX_LEN], &envelope).is_none());
        for i in [0, NONCE_LEN, ENVELOPE_LEN - 1] {
            let mut bad = envelope.clone();
            bad[i] ^= 1;
            assert!(open(&KEY, &prefix, &bad).is_none(), "byte {i}");
        }
        assert!(open(&KEY, &prefix, &envelope[..ENVELOPE_LEN - 1]).is_none());
        assert!(open(&KEY, &prefix, &[envelope.clone(), vec![0]].concat()).is_none());
        // Two seals of the same data differ (fresh nonce).
        assert_ne!(envelope, seal(&KEY, &server(), &invite(), &SECRET));
    }

    #[test]
    fn a_link_holder_cannot_redirect_to_another_server() {
        // Holding k, one can seal any server: the prefix check refuses it
        // unless that server shares the link's 16-byte prefix.
        let other = ServerId::from_bytes([0x99; 32]);
        let forged = seal(&KEY, &other, &invite(), &SECRET);
        assert!(open(&KEY, &prefix_of(&server()), &forged).is_none());
    }

    #[test]
    fn a_publication_round_trips_and_verifies_against_its_author_only() {
        let publication = publication();
        let bytes = publication.encode();
        assert_eq!(bytes.len(), 32 + 32 + 2 + ENVELOPE_LEN + SIG_LEN);
        assert!(bytes.len() <= MAX_PUBLICATION && MAX_PUBLICATION <= 400);
        let decoded = InvitePublication::decode(&bytes).unwrap();
        assert_eq!(decoded, publication);
        let author_key = PublicKey::from_bytes(author().verifying_key().to_bytes());
        assert!(decoded.verify(&author_key));
        let someone =
            PublicKey::from_bytes(SigningKey::from_bytes(&[0x56; 32]).verifying_key().to_bytes());
        assert!(!decoded.verify(&someone));
        assert_eq!(format!("{decoded:?}"), "InvitePublication(..)");
    }

    #[test]
    fn hostile_publications_are_refused() {
        let author_key = PublicKey::from_bytes(author().verifying_key().to_bytes());
        let good = publication().encode();
        // Any field changed: the signature no longer holds.
        for i in [0, 32, 66, good.len() - 1] {
            let mut bad = good.clone();
            bad[i] ^= 1;
            if let Some(p) = InvitePublication::decode(&bad) {
                assert!(!p.verify(&author_key), "byte {i}");
            }
        }
        // Lengths that don't add up, trailing bytes, empty and oversize envelopes.
        assert!(InvitePublication::decode(&good[..good.len() - 1]).is_none());
        assert!(InvitePublication::decode(&[good.clone(), vec![0]].concat()).is_none());
        assert!(InvitePublication::decode(&[]).is_none());
        let mut zero_len = good[..64].to_vec();
        zero_len.extend_from_slice(&[0, 0]);
        zero_len.extend_from_slice(&[0; SIG_LEN]);
        assert!(InvitePublication::decode(&zero_len).is_none());
        let mut too_big = good[..64].to_vec();
        too_big.extend_from_slice(&((MAX_ENVELOPE + 1) as u16).to_be_bytes());
        too_big.extend_from_slice(&vec![0; MAX_ENVELOPE + 1 + SIG_LEN]);
        assert!(InvitePublication::decode(&too_big).is_none());
        assert!(InvitePublication::decode(&vec![0; MAX_PUBLICATION + 1]).is_none());
        // Signing refuses what decoding would.
        assert!(InvitePublication::sign(&author(), invite(), handle(&KEY), vec![]).is_none());
        assert!(InvitePublication::sign(
            &author(),
            invite(),
            handle(&KEY),
            vec![0; MAX_ENVELOPE + 1]
        )
        .is_none());
        assert!(InvitePublication::sign(&author(), invite(), handle(&KEY), vec![0; MAX_ENVELOPE])
            .is_some());
        // A bad author key never verifies (not every 32 bytes is a point).
        let not_a_key = PublicKey::from_bytes([0xff; 32]);
        assert!(!InvitePublication::decode(&good).unwrap().verify(&not_a_key));
    }
}
