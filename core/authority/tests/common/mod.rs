//! Test helpers: actors are numbered, and each number maps to a fixed key.
//! Actor 0 owns the test server.

#![allow(dead_code)]

use authority::{sign_control, sign_genesis, Action, Actor, Record, RecordId};
use records::{PublicKey, ServerId, SigningKey, GENESIS_NONCE_LEN};

pub const NONCE: [u8; GENESIS_NONCE_LEN] = [0x5e; GENESIS_NONCE_LEN];

pub fn key(actor: u8) -> SigningKey {
    SigningKey::from_bytes(&[actor.wrapping_add(1); 32])
}

pub fn actor(n: u8) -> Actor {
    PublicKey::from_bytes(key(n).verifying_key().to_bytes())
}

/// The test server: the one actor 0 creates with [`genesis`].
pub fn server() -> ServerId {
    ServerId::derive(&actor(0), &NONCE)
}

/// The test server's genesis, by actor 0.
pub fn genesis() -> Record {
    sign_genesis(&key(0), NONCE, 0, b"test server").expect("valid genesis")
}

/// A control record by actor `author` on the test server.
pub fn rec(author: u8, seq: u64, deps: Vec<RecordId>, action: Action) -> Record {
    sign_control(&key(author), server(), seq, deps, 0, &action).expect("valid test record")
}
