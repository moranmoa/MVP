//! Rotations `authority` voids after the fact, strict decoding, and the v1
//! wire format pinned with a fixed RNG.

mod common;

use common::*;
use groupkeys::{Applied, Error, Rotation, RotationNeed, WRAP_LEN};
use records::RecordId;

/// One step a peer can see: a rotation arriving, or authority's verdict.
#[derive(Clone)]
enum Event {
    Rotation(Signed),
    Void(Vec<RecordId>),
}

fn run(me: u8, events: &[Event], order: &[usize]) -> Option<RecordId> {
    let mut view = peer(me);
    for i in order {
        match &events[*i] {
            Event::Rotation(rotation) => {
                apply(&mut view, rotation);
            }
            Event::Void(ids) => view.set_voided(ids.iter().copied()),
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

#[test]
fn a_voided_rotation_stops_being_head_in_every_order() {
    // Moderator 2 rotates while the owner revokes them concurrently;
    // authority later voids that rotation.
    let everyone = members(&[1, 2, 3]);
    let mut base = peer(1);
    let first = rotate(&base, 1, &everyone, 1);
    apply(&mut base, &first);
    let revoked = rotate(&base, 2, &everyone, 2);

    let events = [
        Event::Rotation(first.clone()),
        Event::Rotation(revoked.clone()),
        Event::Void(vec![revoked.id]),
    ];
    for order in permutations(events.len()) {
        for me in 1..=3 {
            assert_eq!(run(me, &events, &order), Some(first.id), "order {order:?}");
        }
    }
    // The same head as a peer that never saw the voided rotation at all.
    assert_eq!(run(3, &events[..1], &[0]), Some(first.id));
}

#[test]
fn rotations_built_on_a_voided_one_still_link() {
    // Moderator 3 rotated on top of the voided rotation before the revoke was
    // known. Its key is fresh, so it stays usable and becomes head.
    let everyone = members(&[1, 2, 3]);
    let mut base = peer(1);
    let first = rotate(&base, 1, &everyone, 1);
    apply(&mut base, &first);
    let revoked = rotate(&base, 2, &everyone, 2);
    apply(&mut base, &revoked);
    let on_top = rotate(&base, 3, &everyone, 3);

    let events = [
        Event::Rotation(first.clone()),
        Event::Rotation(revoked.clone()),
        Event::Rotation(on_top.clone()),
        Event::Void(vec![revoked.id]),
    ];
    for order in permutations(events.len()) {
        assert_eq!(run(2, &events, &order), Some(on_top.id), "order {order:?}");
    }
}

#[test]
fn voided_keys_still_read_and_say_so() {
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
    // Alice sent under the rotation before authority voided it.
    let early = alice.encrypt(&everyone, b"sent before the revoke", &mut rng(3)).unwrap();

    for view in [&mut alice, &mut bob] {
        view.set_voided([revoked.id]);
        assert!(view.is_voided(&revoked.id));
        assert_eq!(view.head().unwrap().id(), first.id);
    }
    let opened = bob.decrypt_with_rotation(pk(1), &early).unwrap();
    assert_eq!(opened.plaintext, b"sent before the revoke");
    assert_eq!(opened.rotation, revoked.id);
    assert_eq!(opened.epoch, 1);

    // New messages use the head again, and the next rotation builds on it.
    let late = alice.encrypt(&everyone, b"after", &mut rng(4)).unwrap();
    assert_eq!(bob.decrypt_with_rotation(pk(1), &late).unwrap().rotation, first.id);
    let next = rotate(&alice, 1, &everyone, 5);
    apply(&mut bob, &next);
    let head = bob.head().unwrap();
    assert_eq!((head.id(), head.epoch(), head.prev()), (next.id, 1, Some(first.id)));

    // Authority can change its mind back.
    bob.set_voided([]);
    assert_eq!(bob.head().unwrap().id(), revoked.id.min(next.id));
}

#[test]
fn only_voided_rotations_mean_no_head() {
    let everyone = members(&[1, 2]);
    let mut view = peer(2);
    let first = rotate(&view, 1, &everyone, 1);
    view.set_voided([first.id]);
    apply(&mut view, &first);
    assert!(view.head().is_none());
    assert_eq!(view.rotation_need(&everyone), RotationNeed::Initial);
    assert_eq!(view.encrypt(&everyone, b"x", &mut rng(2)), Err(Error::NoKey));
    // A fresh epoch 0 by an accepted moderator takes over.
    let again = rotate(&view, 1, &everyone, 3);
    apply(&mut view, &again);
    assert_eq!(view.head().unwrap().id(), again.id);
}

#[test]
fn an_unreadable_head_counts_as_missing_me() {
    // Lifted into a record by another author, the body names member 2 but
    // their wrap can't open; they must ask for a rotation.
    let everyone = members(&[1, 2]);
    let lifted = sign(3, rotate(&peer(1), 1, &everyone, 1).body);
    let mut bob = peer(2);
    assert_eq!(apply(&mut bob, &lifted), Applied::WrapInvalid);
    assert_eq!(bob.rotation_need(&everyone), RotationNeed::Add { missing: vec![pk(2)] });
}

/// A valid epoch-0 body for members 1 and 2: header is 74 bytes, then the
/// u16 count, then two wraps.
fn body() -> Vec<u8> {
    let (body, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &members(&[1, 2]), &mut rng(7)).unwrap();
    assert_eq!(body.len(), 76 + 2 * WRAP_LEN);
    body
}

fn decode(body: &[u8]) -> Result<Rotation, Error> {
    Rotation::decode(RecordId::from_bytes([1; 32]), SERVER, pk(1), body)
}

#[test]
fn decode_is_strict() {
    assert!(decode(&body()).is_ok());

    // A huge count with no wraps behind it is refused before any allocation.
    let mut huge = body()[..76].to_vec();
    huge[74..76].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode(&huge), Err(Error::Truncated));

    let mut zero = body()[..76].to_vec();
    zero[74..76].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(decode(&zero), Err(Error::NoRecipients));

    let mut fewer = body();
    fewer[74..76].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(decode(&fewer), Err(Error::TrailingBytes));

    let mut cut = body();
    cut.pop();
    assert_eq!(decode(&cut), Err(Error::Truncated));

    let mut version = body();
    version[0] = 2;
    assert_eq!(decode(&version), Err(Error::UnsupportedVersion(2)));

    let mut tag = body();
    tag[41] = 2;
    assert_eq!(decode(&tag), Err(Error::BadEpochLink));

    let mut epoch_without_prev = body();
    epoch_without_prev[33] = 1;
    assert_eq!(decode(&epoch_without_prev), Err(Error::BadEpochLink));

    // The second wrap repeats the first recipient.
    let mut duplicate = body();
    let (first, second) = duplicate[76..].split_at_mut(WRAP_LEN);
    second[..32].copy_from_slice(&first[..32]);
    assert_eq!(decode(&duplicate), Err(Error::NotCanonical));

    // Swapped wraps are no longer ascending.
    let mut swapped = body();
    let (first, second) = swapped[76..].split_at_mut(WRAP_LEN);
    first.swap_with_slice(second);
    assert_eq!(decode(&swapped), Err(Error::NotCanonical));
}

/// Pins the v1 formats: a fixed RNG must keep producing these exact bytes.
/// Changing a domain string, the KDF or the layout changes them.
#[test]
fn v1_format_is_pinned() {
    let rotation = body();
    let mut view = peer(1);
    let signed = sign(1, rotation.clone());
    apply(&mut view, &signed);
    let ciphertext = view.encrypt(&members(&[1, 2]), b"shalom", &mut rng(8)).unwrap();
    assert_eq!(blake3::hash(&rotation).to_hex().as_str(), GOLDEN_ROTATION);
    assert_eq!(blake3::hash(&ciphertext).to_hex().as_str(), GOLDEN_MESSAGE);
}

const GOLDEN_ROTATION: &str = "412a9501496e7354dd86be6b1391a6e6381bacb04e99def9a672a414a0687eea";
const GOLDEN_MESSAGE: &str = "daf236d32949a10926659431ce2003726b695727fc1ca6b736807f2a6107f5ca";
