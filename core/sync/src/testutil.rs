//! Records for tests, signed with fixed-seed test keys (not secret, never
//! used outside tests).

use records::{
    ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId, SigningKey, GENESIS_NONCE_LEN,
};

const NONCE: [u8; GENESIS_NONCE_LEN] = [7; GENESIS_NONCE_LEN];

/// Fixed-seed test key number `n`. Key 0 owns the test server.
pub fn test_key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n.wrapping_add(1); 32])
}

pub fn public(key: &SigningKey) -> PublicKey {
    PublicKey::from_bytes(key.verifying_key().to_bytes())
}

pub fn server() -> ServerId {
    ServerId::derive(&public(&test_key(0)), &NONCE)
}

pub fn channel() -> ChannelId {
    ChannelId::from_bytes([3; 32])
}

fn sign(scope: Scope, seq: u64, deps: &[RecordId], auth_deps: &[RecordId], kind: Kind, body: Vec<u8>, key: u8) -> Record {
    Draft { server: server(), scope, seq, deps: deps.to_vec(), auth_deps: auth_deps.to_vec(), ts: 0, kind, body }
        .sign(&test_key(key))
        .expect("test record is valid")
}

pub fn genesis() -> Record {
    sign(Scope::Control, 0, &[], &[], Kind::Genesis, [&NONCE[..], b"test server"].concat(), 0)
}

/// A control-log post by the owner.
pub fn post(seq: u64, deps: &[RecordId]) -> Record {
    sign(Scope::Control, seq, deps, &[], Kind::Post, seq.to_le_bytes().to_vec(), 0)
}

/// A channel message by test key 1, with `size` extra body bytes.
pub fn message(seq: u64, deps: &[RecordId], size: usize) -> Record {
    message_with_auth(seq, deps, &[], size)
}

pub fn message_with_auth(seq: u64, deps: &[RecordId], auth_deps: &[RecordId], size: usize) -> Record {
    let body = [seq.to_le_bytes().to_vec(), vec![0; size]].concat();
    sign(Scope::Channel(channel()), seq, deps, auth_deps, Kind::Message, body, 1)
}

/// `n` messages, each on top of the previous one.
pub fn message_chain(n: usize, size: usize) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    for i in 0..n {
        let deps: Vec<RecordId> = out.last().map(|r| vec![r.id()]).unwrap_or_default();
        out.push(message(i as u64, &deps, size));
    }
    out
}
