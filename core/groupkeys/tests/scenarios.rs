mod common;

use std::collections::BTreeSet;

use common::*;
use groupkeys::{
    open, rotation_body_len, seal, Applied, ChannelKeys, Error, MessageContext, Rotation,
    RotationNeed, MAX_MESSAGE_PLAINTEXT, MESSAGE_OVERHEAD, PART_HEADER_LEN, ROTATION_HEADER_LEN,
    WRAPS_PER_PART, WRAP_LEN,
};
use records::{ChannelId, Draft, Kind, PublicKey, Scope, ServerId, SigningKey};

#[test]
fn members_read_each_other() {
    let everyone = members(&[1, 2, 3]);
    let mut alice = peer(1);
    let mut bob = peer(2);
    assert_eq!(alice.rotation_need(&everyone), RotationNeed::Initial);
    let first = rotate(&alice, 1, &everyone, 1);
    assert_eq!(apply(&mut alice, &first), Applied::Key);
    assert_eq!(apply(&mut bob, &first), Applied::Key);
    assert_eq!(apply(&mut bob, &first), Applied::Duplicate);
    assert_eq!(alice.rotation_need(&everyone), RotationNeed::None);

    let ciphertext = alice.encrypt(&everyone, b"shalom", &mut rng(2)).unwrap();
    assert_eq!(ciphertext.len(), 6 + MESSAGE_OVERHEAD);
    assert_eq!(bob.decrypt(pk(1), &ciphertext).unwrap(), b"shalom");
}

#[test]
fn kicked_member_cannot_read_the_next_epoch() {
    let before = members(&[1, 2, 3]);
    let mut owner = peer(1);
    let mut alice = peer(2);
    let mut mallory = peer(3);
    let first = rotate(&owner, 1, &before, 1);
    for view in [&mut owner, &mut alice, &mut mallory] {
        assert_eq!(apply(view, &first), Applied::Key);
    }
    let old = alice.encrypt(&before, b"before the kick", &mut rng(2)).unwrap();
    assert_eq!(mallory.decrypt(pk(2), &old).unwrap(), b"before the kick");

    // Mallory is kicked: authority's member set no longer has them.
    let after = members(&[1, 2]);
    assert_eq!(owner.rotation_need(&after), RotationNeed::Required { removed: vec![pk(3)] });
    assert_eq!(
        alice.encrypt(&after, b"too early", &mut rng(3)),
        Err(Error::RotationRequired { removed: vec![pk(3)] })
    );

    let second = rotate(&owner, 1, &after, 4);
    for view in [&mut owner, &mut alice] {
        assert_eq!(apply(view, &second), Applied::Key);
    }
    assert_eq!(apply(&mut mallory, &second), Applied::NotRecipient);
    assert_eq!(owner.head().unwrap().epoch(), 1);
    assert_eq!(mallory.head().unwrap().id(), second.id);

    let new = alice.encrypt(&after, b"after the kick", &mut rng(5)).unwrap();
    assert_eq!(owner.decrypt(pk(2), &new).unwrap(), b"after the kick");
    assert_eq!(mallory.decrypt(pk(2), &new), Err(Error::NoKey));
    // Mallory still reads what they could read before; that can't be undone.
    assert_eq!(mallory.decrypt(pk(2), &old).unwrap(), b"before the kick");

    // Trying the key Mallory does hold, in the new epoch's context, fails too.
    let first_key = Rotation::decode(SERVER, first.author, first.body())
        .unwrap()
        .unwrap_key(&secret(3))
        .unwrap();
    let ctx = owner.head().unwrap().message_context(pk(2));
    assert_eq!(open(&first_key, &ctx, &new), Err(Error::DecryptFailed));
}

#[test]
fn tampered_ciphertext_ad_or_epoch_is_rejected() {
    let everyone = members(&[1, 2]);
    let mut alice = peer(1);
    let mut bob = peer(2);
    let first = rotate(&alice, 1, &everyone, 1);
    apply(&mut alice, &first);
    apply(&mut bob, &first);
    let ciphertext = alice.encrypt(&everyone, b"hello", &mut rng(2)).unwrap();

    for at in 0..ciphertext.len() {
        let mut tampered = ciphertext.clone();
        tampered[at] ^= 0x01;
        assert!(bob.decrypt(pk(1), &tampered).is_err(), "flip at byte {at} accepted");
    }
    assert!(bob.decrypt(pk(1), &ciphertext[..ciphertext.len() - 1]).is_err());
    let mut longer = ciphertext.clone();
    longer.push(0);
    assert!(bob.decrypt(pk(1), &longer).is_err());

    // The same bytes claimed by another author (bob re-posts alice's
    // ciphertext in a record he signs).
    assert_eq!(bob.decrypt(pk(2), &ciphertext), Err(Error::DecryptFailed));

    // Epoch field rewritten: there's no key for it, and even with the key the
    // associated data no longer matches.
    let mut epoch = ciphertext.clone();
    epoch[1] = 1;
    assert_eq!(bob.decrypt(pk(1), &epoch), Err(Error::NoKey));
    let rotation = bob.head().unwrap().clone();
    let key = rotation.unwrap_key(&secret(2)).unwrap();
    let mut ctx = rotation.message_context(pk(1));
    assert_eq!(open(&key, &ctx, &ciphertext).unwrap(), b"hello");
    ctx.epoch = 1;
    assert_eq!(open(&key, &ctx, &epoch), Err(Error::DecryptFailed));

    // Replayed into another channel or server under the same key.
    let ctx = rotation.message_context(pk(1));
    let other_channel = MessageContext { channel: ChannelId::from_bytes([9; 32]), ..ctx };
    assert_eq!(open(&key, &other_channel, &ciphertext), Err(Error::DecryptFailed));
    let other_server = MessageContext { server: ServerId::from_bytes([9; 32]), ..ctx };
    assert_eq!(open(&key, &other_server, &ciphertext), Err(Error::DecryptFailed));
}

#[test]
fn rotation_for_another_channel_is_refused() {
    let everyone = members(&[1, 2]);
    let elsewhere = ChannelKeys::new(SERVER, ChannelId::from_bytes([7; 32]), secret(1));
    let foreign = rotate(&elsewhere, 1, &everyone, 1);
    let mut here = peer(2);
    let part = &foreign.parts[0];
    assert_eq!(here.apply(part.record, part.author, &part.body), Err(Error::WrongChannel));
    assert!(here.head().is_none());
}

#[test]
fn a_part_signed_by_someone_else_is_refused() {
    // The rotation ID binds the rotator: a part lifted into a record signed
    // by another author is refused outright, and gives nobody a key.
    let everyone = members(&[1, 2]);
    let original = rotate(&peer(1), 1, &everyone, 1);
    let lifted = sign_part(3, original.body().to_vec());
    let mut bob = peer(2);
    let refused = bob.apply(lifted.record, lifted.author, &lifted.body);
    assert_eq!(refused, Err(Error::RotationIdMismatch));
    assert_eq!(bob.held(), 0);
}

#[test]
fn concurrent_rotations_converge() {
    let start = members(&[1, 2, 3, 4, 5]);
    let mut base = peer(1);
    let first = rotate(&base, 1, &start, 1);
    apply(&mut base, &first);

    // Two moderators kick concurrently: 1 kicks 4, 2 kicks 5. Neither has
    // seen the other's kick when they rotate.
    let by_one = rotate(&base, 1, &members(&[1, 2, 3, 5]), 10);
    let by_two = rotate(&base, 2, &members(&[1, 2, 3, 4]), 20);
    let (winner, loser) =
        if by_one.id < by_two.id { (&by_one, &by_two) } else { (&by_two, &by_one) };
    let all = [first.clone(), by_one.clone(), by_two.clone()];

    for order in permutations(all.len()) {
        for me in 1..=5 {
            let mut view = peer(me);
            for i in &order {
                apply(&mut view, &all[*i]);
            }
            assert_eq!(view.head().unwrap().id(), winner.id, "order {order:?}, peer {me}");
        }
    }

    // A message sent on the losing branch before its author saw the winner
    // stays readable to the loser's recipients.
    let mut loser_view = peer(1);
    apply(&mut loser_view, &first);
    apply(&mut loser_view, loser);
    let loser_members: BTreeSet<PublicKey> =
        loser_view.head().unwrap().recipients().copied().collect();
    let author = loser.author;
    let mut author_view =
        ChannelKeys::new(SERVER, CHANNEL, secret(if author == pk(1) { 1 } else { 2 }));
    apply(&mut author_view, &first);
    apply(&mut author_view, loser);
    let on_loser =
        author_view.encrypt(&loser_members, b"on the losing branch", &mut rng(40)).unwrap();

    // Once both kicks are known, the winner still includes the member only
    // the loser removed, so a rotation is forced; it settles on epoch 2.
    let now = members(&[1, 2, 3]);
    let mut view = peer(3);
    for rotation in &all {
        apply(&mut view, rotation);
    }
    let still_in = if winner.id == by_one.id { pk(5) } else { pk(4) };
    assert_eq!(view.rotation_need(&now), RotationNeed::Required { removed: vec![still_in] });
    assert!(matches!(view.encrypt(&now, b"x", &mut rng(41)), Err(Error::RotationRequired { .. })));
    let fix = rotate(&view, 2, &now, 30);
    apply(&mut view, &fix);
    assert_eq!(view.rotation_need(&now), RotationNeed::None);
    assert_eq!(view.head().unwrap().epoch(), 2);
    assert_eq!(view.decrypt(author, &on_loser).unwrap(), b"on the losing branch");
}

#[test]
fn offline_member_catches_up_in_any_order() {
    let mut moderator = peer(1);
    let mut rotations = Vec::new();
    let mut messages = Vec::new();
    for epoch in 0..6u8 {
        // Member 9 is in every epoch; member 10 + epoch comes and goes.
        let current = members(&[1, 9, 10 + epoch]);
        let rotation = rotate(&moderator, 1, &current, u64::from(epoch));
        apply(&mut moderator, &rotation);
        let text = format!("epoch {epoch}");
        let seed = 100 + u64::from(epoch);
        let ciphertext = moderator.encrypt(&current, text.as_bytes(), &mut rng(seed)).unwrap();
        messages.push((text, ciphertext));
        rotations.push(rotation);
    }

    // Member 9 was offline the whole time and receives everything newest
    // first: rotations wait for their prev, then link in one go.
    let mut late = peer(9);
    for rotation in rotations.iter().rev() {
        assert_eq!(apply(&mut late, rotation), Applied::Key);
        if rotation.id != rotations[0].id {
            assert!(late.head().is_none(), "nothing links before epoch 0 arrives");
        }
    }
    assert_eq!(late.head().unwrap().id(), rotations[5].id);
    assert_eq!(late.head().unwrap().epoch(), 5);
    for (text, ciphertext) in &messages {
        assert_eq!(late.decrypt(pk(1), ciphertext).unwrap(), text.as_bytes());
    }
}

#[test]
fn new_member_cannot_read_history_before_joining() {
    let mut moderator = peer(1);
    let mut alice = peer(2);
    let before = members(&[1, 2]);
    let first = rotate(&moderator, 1, &before, 1);
    apply(&mut moderator, &first);
    apply(&mut alice, &first);
    let history = alice.encrypt(&before, b"before carol", &mut rng(2)).unwrap();

    // Carol joins: authority now lists her. Sending still works, but she
    // can't read anything until a rotation adds her.
    let after = members(&[1, 2, 3]);
    assert_eq!(moderator.rotation_need(&after), RotationNeed::Add { missing: vec![pk(3)] });
    assert!(alice.encrypt(&after, b"still allowed", &mut rng(3)).is_ok());
    let joined = rotate(&moderator, 1, &after, 3);
    let mut carol = peer(3);
    for view in [&mut moderator, &mut alice, &mut carol] {
        apply(view, &joined);
    }
    assert_eq!(apply(&mut carol, &first), Applied::NotRecipient);
    assert_eq!(carol.decrypt(pk(2), &history), Err(Error::NoKey));
    let now = alice.encrypt(&after, b"welcome", &mut rng(4)).unwrap();
    assert_eq!(carol.decrypt(pk(2), &now).unwrap(), b"welcome");
}

#[test]
fn author_must_have_held_the_epoch_key() {
    let mut owner = peer(1);
    let first = rotate(&owner, 1, &members(&[1, 2]), 1);
    apply(&mut owner, &first);
    // Member 3 isn't a recipient of the head, yet posts under it (say a
    // recipient leaked the key): rejected even though it would open.
    let rotation = owner.head().unwrap().clone();
    let epoch_key = rotation.unwrap_key(&secret(1)).unwrap();
    let forged = seal(&epoch_key, &rotation.message_context(pk(3)), b"hi", &mut rng(2)).unwrap();
    assert_eq!(owner.decrypt(pk(3), &forged), Err(Error::AuthorNotRecipient));
}

#[test]
fn rotation_epoch_must_follow_prev() {
    let everyone = members(&[1, 2]);
    let mut view = peer(2);
    let first = rotate(&peer(1), 1, &everyone, 1);
    apply(&mut view, &first);
    // Claims epoch 5 on top of epoch 0: stored (its key is kept), never linked.
    let (body, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 5, Some(first.id), &everyone, &mut rng(2))
            .unwrap();
    let jump = sign(1, body);
    assert_eq!(apply(&mut view, &jump), Applied::Key);
    assert_eq!(view.head().unwrap().id(), first.id);

    assert_eq!(
        Rotation::create(SERVER, pk(1), CHANNEL, 0, Some(first.id), &everyone, &mut rng(3))
            .map(|_| ()),
        Err(Error::BadEpochLink)
    );
    assert_eq!(
        Rotation::create(SERVER, pk(1), CHANNEL, 3, None, &everyone, &mut rng(3)).map(|_| ()),
        Err(Error::BadEpochLink)
    );
}

#[test]
fn bad_member_keys_are_refused() {
    // The identity point (all-zero y = 1 encoding) has small order.
    let mut identity = [0; 32];
    identity[0] = 1;
    let weak = PublicKey::from_bytes(identity);
    assert_eq!(
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, [&pk(1), &weak], &mut rng(1)).map(|_| ()),
        Err(Error::BadMemberKey(weak))
    );
    assert_eq!(
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, [], &mut rng(1)).map(|_| ()),
        Err(Error::NoRecipients)
    );
}

#[test]
fn largest_message_fits_a_message_record() {
    let everyone = members(&[1]);
    let mut alice = peer(1);
    let first = rotate(&alice, 1, &everyone, 1);
    apply(&mut alice, &first);
    let ciphertext =
        alice.encrypt(&everyone, &vec![7; MAX_MESSAGE_PLAINTEXT], &mut rng(2)).unwrap();
    assert_eq!(ciphertext.len(), Kind::Message.max_body());
    Draft {
        server: SERVER,
        scope: Scope::Channel(CHANNEL),
        seq: 1,
        deps: vec![],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::Message,
        body: ciphertext,
    }
    .sign(&key(1))
    .unwrap();
    assert_eq!(
        alice.encrypt(&everyone, &vec![7; MAX_MESSAGE_PLAINTEXT + 1], &mut rng(3)),
        Err(Error::MessageTooLarge { len: MAX_MESSAGE_PLAINTEXT + 1, max: MAX_MESSAGE_PLAINTEXT })
    );
}

#[test]
fn large_rotations_split_into_parts() {
    let limit = Kind::KeyRotation.max_body();
    let mut view = peer(1);
    let first = rotate(&view, 1, &members(&[1]), 1);
    apply(&mut view, &first);
    // Two full parts and one more member: three parts.
    let count = 2 * WRAPS_PER_PART + 1;
    let mut many: BTreeSet<PublicKey> = (1..count as u32)
        .map(|n| {
            let mut seed = [0; 32];
            seed[..4].copy_from_slice(&n.to_le_bytes());
            seed[31] = 0xee;
            PublicKey::from_bytes(SigningKey::from_bytes(&seed).verifying_key().to_bytes())
        })
        .collect();
    many.insert(pk(1));
    assert_eq!(many.len(), count);
    let parts = view.rotate(pk(1), &many, &mut rng(2)).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts.iter().map(Vec::len).sum::<usize>(), rotation_body_len(count));
    assert!(parts.iter().all(|part| part.len() <= limit));
    assert!(rotation_body_len(WRAPS_PER_PART + 1) > limit + PART_HEADER_LEN);

    // Each part signs as its own record (records checks its header), and
    // the rotation counts only once every part is in, in any order.
    let signed = sign(1, parts);
    let mut alice = peer(1);
    apply(&mut alice, &first);
    let order = [2, 0, 1];
    for (n, i) in order.iter().enumerate() {
        let part = &signed.parts[*i];
        let (applied, _) = alice.apply(part.record, part.author, &part.body).unwrap();
        let expected = if n + 1 < order.len() { Applied::Incomplete } else { Applied::Key };
        assert_eq!(applied, expected);
        alice.accept(part.record);
    }
    assert_eq!(alice.head().unwrap().id(), signed.id);
    assert_eq!(alice.head().unwrap().recipients().count(), count);
    println!(
        "message overhead {MESSAGE_OVERHEAD} B; rotation part {PART_HEADER_LEN} + \
         {ROTATION_HEADER_LEN} B + {WRAP_LEN} B per member; {WRAPS_PER_PART} members per \
         {limit} B part; 500 members: {} parts, {} B",
        groupkeys::parts_for(500),
        rotation_body_len(500),
    );
}

/// Every ordering of 0..n.
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
