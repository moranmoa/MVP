#![allow(dead_code)]

use std::collections::BTreeSet;

use groupkeys::{Applied, ChannelKeys};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId, SigningKey};

pub const SERVER: ServerId = ServerId::from_bytes([0x5e; 32]);
pub const CHANNEL: ChannelId = ChannelId::from_bytes([0xc4; 32]);

pub fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub fn pk(n: u8) -> PublicKey {
    PublicKey::from_bytes(key(n).verifying_key().to_bytes())
}

pub fn members(ns: &[u8]) -> BTreeSet<PublicKey> {
    ns.iter().map(|n| pk(*n)).collect()
}

pub fn rng(seed: u64) -> ChaCha20Rng {
    ChaCha20Rng::seed_from_u64(seed)
}

pub fn peer(n: u8) -> ChannelKeys {
    ChannelKeys::new(SERVER, CHANNEL, key(n))
}

/// A rotation as it travels: the body of a signed channel record.
#[derive(Clone)]
pub struct Signed {
    pub id: RecordId,
    pub author: PublicKey,
    pub body: Vec<u8>,
}

/// There is no rotation kind in `records` yet, so the tests borrow
/// `Message` for the envelope; that only matters for the ID and signature.
pub fn sign(author: u8, body: Vec<u8>) -> Signed {
    let record = Draft {
        server: SERVER,
        scope: Scope::Channel(CHANNEL),
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::Message,
        body,
    }
    .sign(&key(author))
    .expect("fits the record limits");
    let record = Record::decode(&record.encode()).unwrap();
    Signed { id: record.id(), author: record.author(), body: record.body().to_vec() }
}

/// `rotator` rotates on top of `view`'s head, for `members`.
pub fn rotate(view: &ChannelKeys, rotator: u8, members: &BTreeSet<PublicKey>, seed: u64) -> Signed {
    let body = view.rotate(pk(rotator), members, &mut rng(seed)).unwrap();
    sign(rotator, body)
}

pub fn apply(view: &mut ChannelKeys, rotation: &Signed) -> Applied {
    view.apply(rotation.id, rotation.author, &rotation.body).unwrap()
}
