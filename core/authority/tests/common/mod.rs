//! Test helpers: actors are numbered, and each number maps to a fixed key.

#![allow(dead_code)]

use authority::{sign_control, Action, Actor, Record, RecordId};
use records::{PublicKey, ServerId, SigningKey};

pub const SERVER: ServerId = ServerId::from_bytes([0x5e; 32]);

pub fn key(actor: u8) -> SigningKey {
    SigningKey::from_bytes(&[actor.wrapping_add(1); 32])
}

pub fn actor(n: u8) -> Actor {
    PublicKey::from_bytes(key(n).verifying_key().to_bytes())
}

/// A control record by actor `author` on the test server.
pub fn rec(author: u8, seq: u64, deps: Vec<RecordId>, action: Action) -> Record {
    sign_control(&key(author), SERVER, seq, deps, 0, &action).expect("valid test record")
}
