//! Only rotations `authority` accepts can be head (deny by default), and the
//! head depends on the rotations held and the accepted set, never on the
//! order either arrived in.

mod common;

use common::*;
use groupkeys::{open, Applied, Error, Rotation, RotationNeed};
use records::RecordId;

/// One step a peer can see: a rotation arriving, or authority's current
/// accepted set.
#[derive(Clone)]
enum Event {
    Rotation(Signed),
    /// The part records authority accepts.
    Accepted(Vec<RecordId>),
}

fn run(me: u8, events: &[Event], order: &[usize]) -> Option<RecordId> {
    let mut view = peer(me);
    for i in order {
        match &events[*i] {
            Event::Rotation(rotation) => {
                store(&mut view, rotation);
            }
            Event::Accepted(ids) => view.set_accepted(ids.iter().copied()),
        }
    }
    view.head().map(|head| head.id())
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for rest in permutations(n - 1) {
        for at in 0..=rest.len() {
            let mut order = rest.clone();
            order.insert(at, n - 1);
            out.push(order);
        }
    }
    out
}

/// Security review PoC (PR #9), kept as a regression test: a rotation by a
/// key that may not rotate, wrapped for exactly the real members, must not
/// become head before `authority` accepts it.
#[test]
fn outsider_rotation_is_never_head_unless_accepted() {
    let everyone = members(&[1, 2, 3]);
    let mut alice = peer(1);
    let first = rotate(&alice, 1, &everyone, 1);
    apply(&mut alice, &first);

    // Key 9 isn't a member and may not rotate. It builds on the head, wraps
    // for the real members only, and keeps the epoch key it made.
    let head = alice.head().unwrap();
    let (body, stolen_key) = Rotation::create(
        SERVER,
        pk(9),
        CHANNEL,
        head.epoch() + 1,
        Some(head.id()),
        &everyone,
        &mut rng(99),
    )
    .unwrap();
    let evil = sign(9, body);
    assert_eq!(store(&mut alice, &evil), Applied::Key);

    // Not accepted (authority hasn't ruled, can't, or rejected it): the head
    // stays `first`, and what Alice sends doesn't open under the stolen key.
    assert_eq!(alice.head().unwrap().id(), first.id);
    assert_eq!(alice.rotation_need(&everyone), RotationNeed::None);
    let ciphertext = alice.encrypt(&everyone, b"secret plan", &mut rng(5)).unwrap();
    let mut mallory = peer(9);
    store(&mut mallory, &first);
    store(&mut mallory, &evil);
    let ctx = mallory.rotation(&evil.id).unwrap().message_context(pk(1));
    assert_eq!(open(&stolen_key, &ctx, &ciphertext), Err(Error::DecryptFailed));
    assert_eq!(mallory.decrypt(pk(1), &ciphertext), Err(Error::NoKey));

    // Only if authority accepts it does it become head.
    for record in evil.records() {
        alice.accept(record);
    }
    assert_eq!(alice.head().unwrap().id(), evil.id);
}

#[test]
fn nothing_accepted_means_no_head() {
    let everyone = members(&[1, 2]);
    let mut view = peer(2);
    let first = rotate(&view, 1, &everyone, 1);
    assert_eq!(store(&mut view, &first), Applied::Key);
    assert!(view.head().is_none());
    assert_eq!(view.rotation_need(&everyone), RotationNeed::Initial);
    assert_eq!(view.encrypt(&everyone, b"x", &mut rng(2)), Err(Error::NoKey));
    view.set_accepted(first.records());
    assert_eq!(view.head().unwrap().id(), first.id);
}

#[test]
fn head_is_the_same_in_every_order_of_rotations_and_acceptances() {
    let everyone = members(&[1, 2, 3]);
    let mut base = peer(1);
    let first = rotate(&base, 1, &everyone, 1);
    apply(&mut base, &first);
    let second = rotate(&base, 2, &everyone, 2);
    let evil = rotate(&base, 9, &everyone, 3);

    // Authority accepts `first` and `second`, never `evil`.
    let events = [
        Event::Rotation(first.clone()),
        Event::Rotation(second.clone()),
        Event::Rotation(evil.clone()),
        Event::Accepted([first.records(), second.records()].concat()),
    ];
    for order in permutations(events.len()) {
        for me in 1..=3 {
            assert_eq!(run(me, &events, &order), Some(second.id), "order {order:?}");
        }
    }

    // Authority later voids `second` (say its rotator was revoked
    // concurrently): the verdict changes, and the head falls back the same
    // way in every order.
    let events = [
        Event::Rotation(first.clone()),
        Event::Rotation(second.clone()),
        Event::Rotation(evil.clone()),
        Event::Accepted(first.records()),
    ];
    for order in permutations(events.len()) {
        assert_eq!(run(2, &events, &order), Some(first.id), "order {order:?}");
    }
    // The same head as a peer that never saw the other rotations.
    assert_eq!(run(2, &[events[0].clone(), events[3].clone()], &[0, 1]), Some(first.id));
}

#[test]
fn rotations_built_on_a_non_accepted_one_still_link() {
    // Moderator 3 rotated on top of moderator 2's rotation before authority
    // voided it. Moderator 3's key is fresh, so it stays usable and is head.
    let everyone = members(&[1, 2, 3]);
    let mut base = peer(1);
    let first = rotate(&base, 1, &everyone, 1);
    apply(&mut base, &first);
    let voided = rotate(&base, 2, &everyone, 2);
    apply(&mut base, &voided);
    let on_top = rotate(&base, 3, &everyone, 3);

    let events = [
        Event::Rotation(first.clone()),
        Event::Rotation(voided.clone()),
        Event::Rotation(on_top.clone()),
        Event::Accepted([first.records(), on_top.records()].concat()),
    ];
    for order in permutations(events.len()) {
        assert_eq!(run(2, &events, &order), Some(on_top.id), "order {order:?}");
    }
}

#[test]
fn non_accepted_keys_still_read_and_say_so() {
    let everyone = members(&[1, 2]);
    let mut alice = peer(1);
    let mut bob = peer(2);
    let first = rotate(&alice, 1, &everyone, 1);
    for view in [&mut alice, &mut bob] {
        apply(view, &first);
    }
    let revoked = rotate(&alice, 2, &everyone, 2);
    for view in [&mut alice, &mut bob] {
        apply(view, &revoked);
    }
    // Alice sent under the rotation while authority still accepted it.
    let early = alice.encrypt(&everyone, b"sent before the revoke", &mut rng(3)).unwrap();
    let opened = bob.decrypt_with_rotation(pk(1), &early).unwrap();
    assert_eq!((opened.rotation, opened.accepted), (revoked.id, true));

    for view in [&mut alice, &mut bob] {
        view.set_accepted(first.records());
        assert!(!view.is_accepted(&revoked.id));
        assert_eq!(view.head().unwrap().id(), first.id);
    }
    let opened = bob.decrypt_with_rotation(pk(1), &early).unwrap();
    assert_eq!(opened.plaintext, b"sent before the revoke");
    assert_eq!((opened.rotation, opened.epoch, opened.accepted), (revoked.id, 1, false));

    // New messages use the head again, and the next rotation builds on it.
    let late = alice.encrypt(&everyone, b"after", &mut rng(4)).unwrap();
    let opened = bob.decrypt_with_rotation(pk(1), &late).unwrap();
    assert_eq!((opened.rotation, opened.accepted), (first.id, true));
    let next = rotate(&alice, 1, &everyone, 5);
    apply(&mut bob, &next);
    let head = bob.head().unwrap();
    assert_eq!((head.id(), head.epoch(), head.prev()), (next.id, 1, Some(first.id)));
}

#[test]
fn an_unreadable_head_counts_as_missing_me() {
    // The rotator broke every wrap: the body names member 2 but their wrap
    // can't open; they must ask for a rotation.
    let everyone = members(&[1, 2]);
    let mut parts = peer(1).rotate(pk(1), &everyone, &mut rng(1)).unwrap();
    // Flip the last byte (in the tag) of every wrap, counting from the end.
    let len = parts[0].len();
    for wrap in 0..everyone.len() {
        parts[0][len - 1 - wrap * groupkeys::WRAP_LEN] ^= 1;
    }
    let broken = sign(1, parts);
    let mut bob = peer(2);
    assert_eq!(apply(&mut bob, &broken), Applied::WrapInvalid);
    assert_eq!(bob.rotation_need(&everyone), RotationNeed::Add { missing: vec![pk(2)] });
}
