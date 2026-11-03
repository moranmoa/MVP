use std::collections::BTreeSet;
use std::sync::Arc;

use groupkeys::{MemberSecret, Rotation};
use rand_core::OsRng;
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId, SigningKey};

use super::GroupKeysCipher;
use crate::ports::{CipherError, GroupCipher};

const SERVER: ServerId = ServerId::from_bytes([0x5e; 32]);
const CHANNEL: ChannelId = ChannelId::from_bytes([0xc4; 32]);

/// Fixed-seed test identities.
fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn pk(n: u8) -> PublicKey {
    PublicKey::from_bytes(key(n).verifying_key().to_bytes())
}

fn cipher(n: u8) -> GroupKeysCipher {
    let cipher = GroupKeysCipher::new(Arc::new(MemberSecret::new(&key(n))));
    cipher.set_channels(SERVER, &BTreeSet::from([CHANNEL]));
    cipher
}

fn members(ns: &[u8]) -> BTreeSet<PublicKey> {
    ns.iter().map(|n| pk(*n)).collect()
}

/// What service does with `rotate`'s output: sign every part.
fn sign(author: u8, parts: Vec<Vec<u8>>) -> Vec<Record> {
    parts
        .into_iter()
        .map(|body| {
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
            Record::decode(&record.encode()).unwrap()
        })
        .collect()
}

/// Delivers `records` to every peer and has authority accept them.
fn deliver(peers: &[&GroupKeysCipher], records: &[Record], accepted: &mut BTreeSet<RecordId>) {
    accepted.extend(records.iter().map(Record::id));
    for peer in peers {
        for record in records {
            peer.apply_rotation(record).unwrap();
        }
        peer.set_accepted(SERVER, accepted);
    }
}

#[test]
fn members_read_and_a_non_member_cannot() {
    let (alice, bob, carol) = (cipher(1), cipher(2), cipher(3));
    let everyone = members(&[1, 2]);
    for peer in [&alice, &bob, &carol] {
        peer.membership_changed(SERVER, &everyone);
    }
    assert_eq!(alice.encrypt(SERVER, CHANNEL, b"x"), Err(CipherError::NoKey));

    let mut accepted = BTreeSet::new();
    let parts = alice.rotate(SERVER, CHANNEL, &everyone).unwrap();
    deliver(&[&alice, &bob, &carol], &sign(1, parts), &mut accepted);

    let body = alice.encrypt(SERVER, CHANNEL, b"shalom").unwrap();
    let opened = bob.decrypt(SERVER, CHANNEL, pk(1), &body).unwrap();
    assert_eq!((opened.plaintext.as_slice(), opened.accepted), (&b"shalom"[..], true));
    assert_eq!(carol.decrypt(SERVER, CHANNEL, pk(1), &body), Err(CipherError::NoKey));
    // Re-attributed to another author, or in another channel, it doesn't open.
    assert!(bob.decrypt(SERVER, CHANNEL, pk(2), &body).is_err());
    let elsewhere = ChannelId::from_bytes([7; 32]);
    assert!(bob.decrypt(SERVER, elsewhere, pk(1), &body).is_err());
}

#[test]
fn a_revoked_member_cannot_read_after_the_rotation() {
    let (alice, bob, carol) = (cipher(1), cipher(2), cipher(3));
    let all = members(&[1, 2, 3]);
    let mut accepted = BTreeSet::new();
    for peer in [&alice, &bob, &carol] {
        peer.membership_changed(SERVER, &all);
    }
    let parts = alice.rotate(SERVER, CHANNEL, &all).unwrap();
    deliver(&[&alice, &bob, &carol], &sign(1, parts), &mut accepted);
    let before = alice.encrypt(SERVER, CHANNEL, b"before").unwrap();

    // Carol is revoked: nobody sends until a moderator rotates.
    let remaining = members(&[1, 2]);
    for peer in [&alice, &bob, &carol] {
        peer.membership_changed(SERVER, &remaining);
    }
    assert_eq!(alice.encrypt(SERVER, CHANNEL, b"x"), Err(CipherError::RotationRequired));
    let parts = alice.rotate(SERVER, CHANNEL, &remaining).unwrap();
    deliver(&[&alice, &bob, &carol], &sign(1, parts), &mut accepted);

    let after = alice.encrypt(SERVER, CHANNEL, b"after").unwrap();
    assert_eq!(bob.decrypt(SERVER, CHANNEL, pk(1), &after).unwrap().plaintext, b"after");
    assert_eq!(carol.decrypt(SERVER, CHANNEL, pk(1), &after), Err(CipherError::NoKey));
    // What Carol already had stays readable to her; that can't be undone.
    assert_eq!(carol.decrypt(SERVER, CHANNEL, pk(1), &before).unwrap().plaintext, b"before");
}

#[test]
fn junk_rotations_are_capped_and_never_become_the_key() {
    let (alice, bob) = (cipher(1), cipher(2));
    let everyone = members(&[1, 2]);
    let mut accepted = BTreeSet::new();
    for peer in [&alice, &bob] {
        peer.membership_changed(SERVER, &everyone);
    }
    let parts = alice.rotate(SERVER, CHANNEL, &everyone).unwrap();
    deliver(&[&alice, &bob], &sign(1, parts), &mut accepted);

    // Key 9 floods signed rotations that authority never accepts.
    let mut evicted = 0;
    for _ in 0..100 {
        let (parts, _) =
            Rotation::create(SERVER, pk(9), CHANNEL, 0, None, &everyone, &mut OsRng).unwrap();
        for record in sign(9, parts) {
            evicted += bob.apply_rotation(&record).unwrap().len();
        }
    }
    assert_eq!(evicted, 100 - groupkeys::MAX_UNACCEPTED);

    // Messages still use the accepted key and stay trusted.
    let body = alice.encrypt(SERVER, CHANNEL, b"still here").unwrap();
    let opened = bob.decrypt(SERVER, CHANNEL, pk(1), &body).unwrap();
    assert_eq!((opened.plaintext.as_slice(), opened.accepted), (&b"still here"[..], true));
}

#[test]
fn only_key_rotation_records_are_taken_in() {
    let bob = cipher(2);
    let record = Draft {
        server: SERVER,
        scope: Scope::Channel(CHANNEL),
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::Message,
        body: vec![1, 2, 3],
    }
    .sign(&key(1))
    .unwrap();
    assert_eq!(bob.apply_rotation(&record), Err(CipherError::Rejected));
}

#[test]
fn locked_until_unlocked_and_lock_drops_everything() {
    let alice = GroupKeysCipher::locked();
    alice.set_channels(SERVER, &BTreeSet::from([CHANNEL]));
    let everyone = members(&[1, 2]);
    alice.membership_changed(SERVER, &everyone);
    assert_eq!(alice.rotate(SERVER, CHANNEL, &everyone), Err(CipherError::Unavailable));
    assert_eq!(alice.encrypt(SERVER, CHANNEL, b"x"), Err(CipherError::Unavailable));

    alice.unlock(Arc::new(MemberSecret::new(&key(1))));
    let mut accepted = BTreeSet::new();
    let parts = alice.rotate(SERVER, CHANNEL, &everyone).unwrap();
    deliver(&[&alice], &sign(1, parts), &mut accepted);
    assert!(alice.encrypt(SERVER, CHANNEL, b"x").is_ok());

    alice.lock();
    assert_eq!(alice.encrypt(SERVER, CHANNEL, b"x"), Err(CipherError::Unavailable));
}

#[test]
fn debug_never_prints_the_secret() {
    let scalar = hex::encode(key(1).to_scalar_bytes());
    let secret = MemberSecret::new(&key(1));
    let cipher = GroupKeysCipher::new(Arc::new(MemberSecret::new(&key(1))));
    for shown in [format!("{secret:?}"), format!("{cipher:?}"), format!("{secret:#?}")] {
        assert!(!shown.contains(&scalar), "{shown}");
        assert!(!shown.contains(&scalar[..16]), "{shown}");
    }
}

#[test]
fn unknown_channels_create_no_state() {
    let (alice, bob) = (cipher(1), cipher(2));
    let everyone = members(&[1, 2]);
    bob.membership_changed(SERVER, &everyone);
    // A member names 1,000 channels authority doesn't know.
    for n in 0..1_000u32 {
        let mut id = [0; 32];
        id[..4].copy_from_slice(&n.to_le_bytes());
        let channel = ChannelId::from_bytes(id);
        let (parts, _) =
            Rotation::create(SERVER, pk(1), channel, 0, None, &everyone, &mut OsRng).unwrap();
        let record = Draft {
            server: SERVER,
            scope: Scope::Channel(channel),
            seq: 0,
            deps: vec![],
            auth_deps: vec![],
            ts: 0,
            kind: Kind::KeyRotation,
            body: parts.into_iter().next().unwrap(),
        }
        .sign(&key(1))
        .unwrap();
        assert_eq!(bob.apply_rotation(&record), Err(CipherError::Rejected));
        assert!(bob.decrypt(SERVER, channel, pk(1), b"x").is_err());
        assert_eq!(bob.encrypt(SERVER, channel, b"x"), Err(CipherError::NoKey));
        assert_eq!(bob.rotate(SERVER, channel, &everyone), Err(CipherError::Rejected));
    }
    assert_eq!(bob.channels_held(), 0);
    // Another server's channels aren't known either.
    let other = ServerId::from_bytes([1; 32]);
    assert_eq!(alice.encrypt(other, CHANNEL, b"x"), Err(CipherError::NoKey));
    assert_eq!(alice.channels_held(), 0);
}

#[test]
fn a_channel_authority_drops_loses_its_keys() {
    let (alice, bob) = (cipher(1), cipher(2));
    let everyone = members(&[1, 2]);
    let mut accepted = BTreeSet::new();
    for peer in [&alice, &bob] {
        peer.membership_changed(SERVER, &everyone);
    }
    let parts = alice.rotate(SERVER, CHANNEL, &everyone).unwrap();
    deliver(&[&alice, &bob], &sign(1, parts), &mut accepted);
    let body = alice.encrypt(SERVER, CHANNEL, b"x").unwrap();
    assert_eq!(bob.channels_held(), 1);

    bob.set_channels(SERVER, &BTreeSet::new());
    assert_eq!(bob.channels_held(), 0);
    assert!(bob.decrypt(SERVER, CHANNEL, pk(1), &body).is_err());
    // Known again: a fresh, empty view; the rotations must be applied again.
    bob.set_channels(SERVER, &BTreeSet::from([CHANNEL]));
    assert_eq!(bob.decrypt(SERVER, CHANNEL, pk(1), &body), Err(CipherError::NoKey));
}
