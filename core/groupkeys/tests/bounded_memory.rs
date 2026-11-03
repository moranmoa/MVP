//! GAP-8 (#19) with rotation parts: rotations nobody accepted can't grow a
//! peer's memory past 64 rotations or 2 MiB of parts, a complete honest
//! rotation always fits, and parts that contradict each other get the
//! rotation refused.

mod common;

use std::collections::BTreeSet;

use common::*;
use groupkeys::{
    rotation_id, Applied, ChannelKeys, Rotation, MAX_PARTS, MAX_ROTATION_BYTES, MAX_UNACCEPTED,
    MAX_UNACCEPTED_BYTES, WRAPS_PER_PART,
};
use records::{PublicKey, RecordId};

/// A rotation by key 9, which may not rotate, on top of `prev` at `epoch`.
fn junk(prev: Option<RecordId>, epoch: u64, to: &BTreeSet<PublicKey>, seed: u64) -> Signed {
    let (parts, _) =
        Rotation::create(SERVER, pk(9), CHANNEL, epoch, prev, to, &mut rng(seed)).unwrap();
    sign(9, parts)
}

/// A rotation of `parts` full parts by `rotator`, built by hand: well-formed,
/// with the right rotation ID, but the wraps are filler (nobody gets a key).
/// Cheap enough to flood with. Record IDs are made up; `groupkeys` doesn't
/// check signatures, `records` does.
fn bulk(rotator: u8, prev: Option<RecordId>, epoch: u64, parts: usize, seed: u64) -> Signed {
    let mut header = vec![groupkeys::VERSION];
    header.extend_from_slice(CHANNEL.as_bytes());
    header.extend_from_slice(&epoch.to_le_bytes());
    match prev {
        None => header.push(0),
        Some(prev) => {
            header.push(1);
            header.extend_from_slice(prev.as_bytes());
        }
    }
    let mut ephemeral = [0; 32];
    ephemeral[..8].copy_from_slice(&seed.to_le_bytes());
    header.extend_from_slice(&ephemeral);
    let id = rotation_id(pk(rotator), &header);
    let parts = (0..parts)
        .map(|index| {
            let mut body = id.as_bytes().to_vec();
            body.extend_from_slice(&[index as u8, parts as u8]);
            body.extend_from_slice(&header);
            body.extend_from_slice(&(WRAPS_PER_PART as u16).to_le_bytes());
            for n in 0..WRAPS_PER_PART {
                let mut recipient = [0; 32];
                recipient[..4]
                    .copy_from_slice(&((index * WRAPS_PER_PART + n) as u32).to_be_bytes());
                body.extend_from_slice(&recipient);
                body.extend_from_slice(&[0; 48]);
            }
            let record = RecordId::from_bytes(*blake3::hash(&body).as_bytes());
            SignedPart { record, author: pk(rotator), body }
        })
        .collect();
    Signed { id, author: pk(rotator), parts }
}

fn within_limits(view: &ChannelKeys) {
    assert!(view.on_probation() <= MAX_UNACCEPTED);
    assert!(view.probation_bytes() <= MAX_UNACCEPTED_BYTES);
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
        within_limits(&bob);
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
fn junk_of_32_parts_stays_within_2_mib() {
    let everyone = members(&[1, 2]);
    let mut bob = peer(2);
    let first = rotate(&bob, 1, &everyone, 1);
    apply(&mut bob, &first);

    let mut evicted = 0;
    for seed in 0..64 {
        let rotation = bulk(9, Some(first.id), 1, MAX_PARTS, seed);
        for part in &rotation.parts {
            evicted += bob.apply(part.record, part.author, &part.body).unwrap().1.len();
            within_limits(&bob);
        }
    }
    // A full 32-part rotation is 524,224 B: four fit in 2 MiB, five don't.
    assert_eq!(bob.on_probation(), 4);
    assert_eq!(evicted, (64 - 4) * MAX_PARTS);
    assert_eq!(bob.head().unwrap().id(), first.id);
    println!("on probation: {} rotations, {} B", bob.on_probation(), bob.probation_bytes());
}

#[test]
fn a_complete_honest_rotation_always_fits() {
    let everyone = members(&[1, 2]);
    let mut bob = peer(2);
    let first = rotate(&bob, 1, &everyone, 1);
    apply(&mut bob, &first);
    for seed in 0..8 {
        push(&mut bob, &bulk(9, Some(first.id), 1, MAX_PARTS, seed));
    }

    // The largest honest rotation arrives, one part at a time, while the
    // attacker keeps sending a part of its own between each of ours. Its
    // verdict comes after the last part.
    let honest = bulk(1, Some(first.id), 1, MAX_PARTS, 1_000);
    let more = bulk(9, Some(first.id), 1, MAX_PARTS, 2_000);
    assert!(honest.parts.iter().map(|part| part.body.len()).sum::<usize>() <= MAX_ROTATION_BYTES);
    for (ours, theirs) in honest.parts.iter().zip(&more.parts) {
        let (_, evicted) = bob.apply(ours.record, ours.author, &ours.body).unwrap();
        assert!(!evicted.iter().any(|record| honest.records().contains(record)));
        let (_, evicted) = bob.apply(theirs.record, theirs.author, &theirs.body).unwrap();
        assert!(!evicted.iter().any(|record| honest.records().contains(record)));
        within_limits(&bob);
    }
    assert!(bob.rotation(&honest.id).is_some());
    bob.set_accepted([first.records(), honest.records()].concat());
    assert_eq!(bob.head().unwrap().id(), honest.id);
}

#[test]
fn contradicting_parts_get_the_rotation_refused() {
    let everyone = members(&[1, 2]);
    let mut bob = peer(2);
    let first = rotate(&bob, 1, &everyone, 1);
    apply(&mut bob, &first);
    let second = rotate(&bob, 1, &everyone, 2);
    apply(&mut bob, &second);
    assert_eq!(bob.head().unwrap().id(), second.id);

    // The rotator signs a second record for the same part: a changed wrap.
    let mut other = second.body().to_vec();
    *other.last_mut().unwrap() ^= 1;
    let twin = sign_part(1, other);
    assert_eq!(bob.apply(twin.record, twin.author, &twin.body).unwrap().0, Applied::Refused);
    assert_eq!(bob.head().unwrap().id(), first.id);
    assert!(!bob.is_accepted(&second.id));
    // Accepting the twin as well changes nothing.
    bob.accept(twin.record);
    assert_eq!(bob.head().unwrap().id(), first.id);

    // Same in the other order, and for a part that claims a different count.
    let mut carol = peer(2);
    apply(&mut carol, &first);
    let mut count = second.body().to_vec();
    count[33] = 2;
    let split = sign_part(1, count);
    assert_eq!(
        carol.apply(split.record, split.author, &split.body).unwrap().0,
        Applied::Incomplete
    );
    assert_eq!(apply(&mut carol, &second), Applied::Refused);
    assert_eq!(carol.head().unwrap().id(), first.id);
}

#[test]
fn evicted_rotations_are_returned_and_link_on_redelivery() {
    let everyone = members(&[1, 2]);
    let mut source = peer(1);
    let first = rotate(&source, 1, &everyone, 1);
    apply(&mut source, &first);
    let second = rotate(&source, 1, &everyone, 2);

    let mut bob = ChannelKeys::with_limits(SERVER, CHANNEL, secret(2), 2, 0);
    // `second` arrives before its prev and before authority's verdict: it
    // waits, on probation, and two pieces of junk push it out.
    assert_eq!(store(&mut bob, &second), Applied::Key);
    assert!(push(&mut bob, &junk(None, 0, &everyone, 3)).1.is_empty());
    assert_eq!(push(&mut bob, &junk(None, 0, &everyone, 4)).1, second.records());
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

    let mut bob = ChannelKeys::with_limits(SERVER, CHANNEL, secret(2), 1, 0);
    // Authority's verdict comes first, then `second`, waiting on `first`.
    bob.set_accepted([first.records(), second.records()].concat());
    store(&mut bob, &second);
    for seed in 0..20 {
        let (_, evicted) = push(&mut bob, &junk(None, 0, &everyone, seed));
        assert!(!evicted.contains(&second.records()[0]));
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
    let mut bob = ChannelKeys::with_limits(SERVER, CHANNEL, secret(2), 2, 0);
    apply(&mut bob, &first);
    store(&mut bob, &unaccepted);
    apply(&mut bob, &on_top);
    assert_eq!(bob.on_probation(), 0);
    for seed in 10..20 {
        let (_, evicted) = push(&mut bob, &junk(None, 0, &everyone, seed));
        assert!(!evicted.contains(&unaccepted.records()[0]));
    }
    assert!(bob.has_key(&unaccepted.id));
    assert_eq!(bob.head().unwrap().id(), on_top.id);

    // The same, but authority's verdict on `on_top` comes before either
    // rotation: `unaccepted` is protected as soon as `on_top` arrives.
    let mut dave = ChannelKeys::with_limits(SERVER, CHANNEL, secret(2), 1, 0);
    apply(&mut dave, &first);
    dave.set_accepted([first.records(), on_top.records()].concat());
    store(&mut dave, &unaccepted);
    store(&mut dave, &on_top);
    assert_eq!(dave.on_probation(), 0);

    // Neither accepted: evicting the parent takes the child with it.
    let mut carol = ChannelKeys::with_limits(SERVER, CHANNEL, secret(3), 2, 0);
    apply(&mut carol, &first);
    store(&mut carol, &unaccepted);
    store(&mut carol, &on_top);
    assert_eq!(carol.on_probation(), 2);
    let (_, evicted) = push(&mut carol, &junk(None, 0, &everyone, 20));
    assert_eq!(evicted, [unaccepted.records(), on_top.records()].concat());
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
        evicted += push(&mut bob, &rotation).1.len();
        within_limits(&bob);
        assert!(bob.held() <= 1 + MAX_UNACCEPTED);
        assert_eq!(bob.head().unwrap().id(), first.id);
    }
    assert_eq!(evicted + bob.held(), 1_001);
    assert_eq!(bob.held() - bob.on_probation(), 1);
}
