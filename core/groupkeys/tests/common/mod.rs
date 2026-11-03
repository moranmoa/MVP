#![allow(dead_code)]

use std::collections::BTreeSet;

use std::sync::Arc;

use groupkeys::{Applied, ChannelKeys, MemberSecret};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId, SigningKey};

pub const SERVER: ServerId = ServerId::from_bytes([0x5e; 32]);
pub const CHANNEL: ChannelId = ChannelId::from_bytes([0xc4; 32]);

pub fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

/// The X25519 secret of key `n`, as a client shares it between channels.
pub fn secret(n: u8) -> Arc<MemberSecret> {
    Arc::new(MemberSecret::new(&key(n)))
}

pub fn pk(n: u8) -> PublicKey {
    PublicKey::from_bytes(key(n).verifying_key().to_bytes())
}

pub fn members(ns: &[u8]) -> BTreeSet<PublicKey> {
    ns.iter().map(|n| pk(*n)).collect()
}

/// A fixed-seed ChaCha20 RNG for reproducible tests and the golden vectors.
pub fn rng(seed: u64) -> TestRng {
    TestRng(ChaCha20Rng::seed_from_u64(seed))
}

/// `rand_chacha` 0.9 implements the `rand_core` 0.9 traits; `groupkeys`
/// takes `rand_core` 0.6 ones (as `chacha20poly1305` and `ed25519-dalek` 2
/// do). This passes the same ChaCha20 stream through.
pub struct TestRng(ChaCha20Rng);

impl rand_core::RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        rand_chacha::rand_core::RngCore::next_u32(&mut self.0)
    }

    fn next_u64(&mut self) -> u64 {
        rand_chacha::rand_core::RngCore::next_u64(&mut self.0)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        rand_chacha::rand_core::RngCore::fill_bytes(&mut self.0, dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl rand_core::CryptoRng for TestRng {}

pub fn peer(n: u8) -> ChannelKeys {
    ChannelKeys::new(SERVER, CHANNEL, secret(n))
}

/// One part as it travels: the body of a signed `KeyRotation` record.
#[derive(Clone)]
pub struct SignedPart {
    pub record: RecordId,
    pub author: PublicKey,
    pub body: Vec<u8>,
}

/// A rotation as it travels: all its parts, signed.
#[derive(Clone)]
pub struct Signed {
    /// The rotation ID.
    pub id: RecordId,
    pub author: PublicKey,
    pub parts: Vec<SignedPart>,
}

impl Signed {
    /// The records `authority` would accept for this rotation.
    pub fn records(&self) -> Vec<RecordId> {
        self.parts.iter().map(|part| part.record).collect()
    }

    /// The body of a rotation that fits in one part.
    pub fn body(&self) -> &[u8] {
        assert_eq!(self.parts.len(), 1, "a one-part rotation");
        &self.parts[0].body
    }
}

/// Signs one part body in a `KeyRotation` record by key `author`.
pub fn sign_part(author: u8, body: Vec<u8>) -> SignedPart {
    let record = Draft {
        server: SERVER,
        scope: Scope::Channel(CHANNEL),
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::KeyRotation,
        body,
    }
    .sign(&key(author))
    .expect("fits the record limits");
    let record = Record::decode(&record.encode()).unwrap();
    SignedPart { record: record.id(), author: record.author(), body: record.body().to_vec() }
}

/// Signs every part body of a rotation by key `author`.
pub fn sign(author: u8, parts: Vec<Vec<u8>>) -> Signed {
    let id = RecordId::from_bytes(parts[0][..32].try_into().unwrap());
    Signed {
        id,
        author: pk(author),
        parts: parts.into_iter().map(|body| sign_part(author, body)).collect(),
    }
}

/// `rotator` rotates on top of `view`'s head, for `members`.
pub fn rotate(view: &ChannelKeys, rotator: u8, members: &BTreeSet<PublicKey>, seed: u64) -> Signed {
    let parts = view.rotate(pk(rotator), members, &mut rng(seed)).unwrap();
    sign(rotator, parts)
}

/// Applies a rotation that `authority` accepts, as every rotation in the
/// scenario tests is. Returns what the last part did.
pub fn apply(view: &mut ChannelKeys, rotation: &Signed) -> Applied {
    let applied = store(view, rotation);
    for record in rotation.records() {
        view.accept(record);
    }
    applied
}

/// Applies a rotation `authority` hasn't accepted (yet). Returns what the
/// last part did.
pub fn store(view: &mut ChannelKeys, rotation: &Signed) -> Applied {
    push(view, rotation).0
}

/// Applies every part of a rotation; returns what the last part did and
/// every record evicted on the way.
pub fn push(view: &mut ChannelKeys, rotation: &Signed) -> (Applied, Vec<RecordId>) {
    let mut evicted = Vec::new();
    let mut applied = Applied::Duplicate;
    for part in &rotation.parts {
        let (last, out) = view.apply(part.record, part.author, &part.body).unwrap();
        applied = last;
        evicted.extend(out);
    }
    (applied, evicted)
}
