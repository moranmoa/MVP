//! GAP-8 (#19): rotations nobody accepted can't grow a peer's memory without
//! bound, and what matters survives the eviction or comes back with
//! redelivery.

mod common;

use std::collections::BTreeSet;

use common::*;
use groupkeys::{Applied, ChannelKeys, Rotation, MAX_UNACCEPTED};
use records::{PublicKey, RecordId};

/// A rotation by key 9, which may not rotate, on top of `prev` at `epoch`.
fn junk(prev: Option<RecordId>, epoch: u64, to: &BTreeSet<PublicKey>, seed: u64) -> Signed {
    let (body, _) =
        Rotation::create(SERVER, pk(9), CHANNEL, epoch, prev, to, &mut rng(seed)).unwrap();
    sign(9, body)
}

/// Applies a rotation and returns what was evicted to make room.
fn push(view: &mut ChannelKeys, rotation: &Signed) -> Vec<RecordId> {
    view.apply(rotation.id, rotation.author, &rotation.body).unwrap().1
}

#[test]
fn a_flood_of_unaccepted_rotations_stays_bounded() {
    let everyone = members(&[1, 2]);
    let mut bob = peer(2);
    let first = rotate(&bob, 1, &everyone, 1);
    apply(&mut bob, &first);
    let second = rotate(&bob, 1, &everyone, 2);
    apply(&mut bob, &second);

    // Every shape of junk: leaves on the head wrapped for Bob (he gets a
    // key) or not, orphans whose prev never arrives, and a wrong epoch.
    let outsiders = members(&[7, 8]);
    for n in 0..400u64 {
        let rotation = match n % 4 {
            0 => junk(Some(second.id), 2, &everyone, 100 + n),
            1 => junk(Some(second.id), 2, &outsiders, 100 + n),
            2 => junk(Some(RecordId::from_bytes([n as u8; 32])), 7, &everyone, 100 + n),
            _ => junk(Some(first.id), 5, &everyone, 100 + n),
        };
        store(&mut bob, &rotation);
        assert!(bob.on_probation() <= MAX_UNACCEPTED);
        assert!(bob.held() <= 2 + MAX_UNACCEPTED);
    }
    assert_eq!(bob.on_probation(), MAX_UNACCEPTED);

    // The accepted chain is untouched and still sends and reads.
    assert_eq!(bob.head().unwrap().id(), second.id);
    assert!(bob.has_key(&first.id) && bob.has_key(&second.id));
    let mut alice = peer(1);
    apply(&mut alice, &first);
    apply(&mut alice, &second);
    let ciphertext = alice.encrypt(&everyone, b"still here", &mut rng(3)).unwrap();
    assert_eq!(bob.decrypt(pk(1), &ciphertext).unwrap(), b"still here");
}

#[test]
fn evicted_rotations_are_returned_and_link_on_redelivery() {
    let everyone = members(&[1, 2]);
    let mut source = peer(1);
    let first = rotate(&source, 1, &everyone, 1);
    apply(&mut source, &first);
    let second = rotate(&source, 1, &everyone, 2);

    let mut bob = ChannelKeys::with_limit(SERVER, CHANNEL, secret(2), 2);
    // `second` arrives before its prev and before authority's verdict: it
    // waits, on probation, and two pieces of junk push it out.
    assert_eq!(store(&mut bob, &second), Applied::Key);
    let evicted = push(&mut bob, &junk(None, 0, &everyone, 3));
    assert!(evicted.is_empty());
    let late = junk(None, 0, &everyone, 4);
    let evicted = push(&mut bob, &late);
    assert_eq!(evicted, vec![second.id]);
    assert!(bob.rotation(&second.id).is_none() && !bob.has_key(&second.id));

    // Its prev arrives; sync delivers `second` again, and it links.
    apply(&mut bob, &first);
    assert_eq!(bob.head().unwrap().id(), first.id);
    assert_eq!(apply(&mut bob, &second), Applied::Key);
    assert_eq!(bob.head().unwrap().id(), second.id);
}

#[test]
fn accepted_rotations_are_never_evicted_even_while_waiting() {
    let everyone = members(&[1, 2]);
    let mut source = peer(1);
    let first = rotate(&source, 1, &everyone, 1);
    apply(&mut source, &first);
    let second = rotate(&source, 1, &everyone, 2);

    let mut bob = ChannelKeys::with_limit(SERVER, CHANNEL, secret(2), 1);
    // Authority's verdict comes first, then `second`, waiting on `first`.
    bob.set_accepted([first.id, second.id]);
    store(&mut bob, &second);
    for seed in 0..20 {
        let evicted = push(&mut bob, &junk(None, 0, &everyone, seed));
        assert!(!evicted.contains(&second.id));
    }
    assert_eq!(bob.on_probation(), 1);
    store(&mut bob, &first);
    assert_eq!(bob.head().unwrap().id(), second.id);
}

#[test]
fn ancestors_of_accepted_rotations_are_kept() {
    // Moderator 2's rotation was never accepted, but moderator 3 rotated on
    // top of it and that one is: the parent must stay for the chain.
    let everyone = members(&[1, 2, 3]);
    let mut source = peer(1);
    let first = rotate(&source, 1, &everyone, 1);
    apply(&mut source, &first);
    let unaccepted = rotate(&source, 2, &everyone, 2);
    apply(&mut source, &unaccepted);
    let on_top = rotate(&source, 3, &everyone, 3);

    // Room for both while authority's verdict on `on_top` is on its way.
    let mut bob = ChannelKeys::with_limit(SERVER, CHANNEL, secret(2), 2);
    apply(&mut bob, &first);
    store(&mut bob, &unaccepted);
    apply(&mut bob, &on_top);
    assert_eq!(bob.on_probation(), 0);
    for seed in 10..20 {
        let evicted = push(&mut bob, &junk(None, 0, &everyone, seed));
        assert!(!evicted.contains(&unaccepted.id));
    }
    assert!(bob.has_key(&unaccepted.id));
    assert_eq!(bob.head().unwrap().id(), on_top.id);

    // The same, but authority's verdict on `on_top` comes before either
    // rotation: `unaccepted` is protected as soon as `on_top` arrives.
    let mut dave = ChannelKeys::with_limit(SERVER, CHANNEL, secret(2), 1);
    apply(&mut dave, &first);
    dave.accept(on_top.id);
    store(&mut dave, &unaccepted);
    store(&mut dave, &on_top);
    assert_eq!(dave.on_probation(), 0);

    // Neither accepted: evicting the parent takes the child with it.
    let mut carol = ChannelKeys::with_limit(SERVER, CHANNEL, secret(3), 2);
    apply(&mut carol, &first);
    store(&mut carol, &unaccepted);
    store(&mut carol, &on_top);
    assert_eq!(carol.on_probation(), 2);
    let evicted = push(&mut carol, &junk(None, 0, &everyone, 20));
    assert_eq!(evicted, vec![unaccepted.id, on_top.id]);
    assert_eq!(carol.head().unwrap().id(), first.id);
}

#[test]
fn a_chain_of_junk_on_the_head_stays_bounded() {
    // Security review of #51: each junk rotation builds on the previous one,
    // starting from the accepted head. Building on something mustn't protect
    // it, or the chain grows without limit.
    let everyone = members(&[1, 2]);
    let mut bob = peer(2);
    let first = rotate(&bob, 1, &everyone, 1);
    apply(&mut bob, &first);

    let mut tip = first.id;
    let mut epoch = 0;
    let mut evicted = 0;
    for seed in 0..1_000u64 {
        // The attacker keeps building on what it sent, whether or not we
        // still hold it.
        epoch += 1;
        let rotation = junk(Some(tip), epoch, &everyone, 1_000 + seed);
        tip = rotation.id;
        evicted += push(&mut bob, &rotation).len();
        assert!(bob.on_probation() <= MAX_UNACCEPTED);
        assert!(bob.held() <= 1 + MAX_UNACCEPTED);
        assert_eq!(bob.head().unwrap().id(), first.id);
    }
    assert_eq!(evicted + bob.held(), 1_001);
    assert_eq!(bob.held() - bob.on_probation(), 1);
}
