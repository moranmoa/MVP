//! Channel records judged against the control log (GAP-2, GAP-5): who may
//! post, edit, delete and rotate, pending `auth_deps`, and voiding by the
//! cut of a kick or revoke.

mod common;

use std::collections::HashMap;

use authority::{
    check_channel, check_channel_with, message_text_ok, privileged_only, Action, ChannelGraph,
    ChannelId, Cut, Record, RecordId, Refusal, Resolver, Role, Verdict, MAX_MESSAGE_CODE_POINTS,
};
use common::{actor, founded, key, rec, server};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use records::{Draft, Kind, Scope};

const GENERAL: ChannelId = ChannelId::from_bytes([5; 32]);
const RANDOM: ChannelId = ChannelId::from_bytes([6; 32]);

/// The caller's channel store: records by ID, ancestry through deps.
#[derive(Default)]
struct Store(HashMap<RecordId, Record>);

impl Store {
    fn add(&mut self, record: &Record) -> RecordId {
        self.0.insert(record.id(), record.clone());
        record.id()
    }
}

impl ChannelGraph for Store {
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool {
        let mut stack = vec![of];
        while let Some(next) = stack.pop() {
            if next == ancestor {
                return true;
            }
            if let Some(record) = self.0.get(&next) {
                stack.extend_from_slice(record.deps());
            }
        }
        false
    }

    fn author(&self, id: RecordId) -> Option<authority::PublicKey> {
        self.0.get(&id).map(Record::author)
    }
}

fn resolver(records: &[Record]) -> Resolver {
    let mut resolver = Resolver::new(server());
    for record in records {
        resolver.insert(record.clone());
    }
    resolver
}

fn channel_record(
    author: u8,
    seq: u64,
    channel: ChannelId,
    deps: Vec<RecordId>,
    auth_deps: Vec<RecordId>,
    kind: Kind,
    body: Vec<u8>,
) -> Record {
    Draft {
        server: server(),
        scope: Scope::Channel(channel),
        seq,
        deps,
        auth_deps,
        ts: 0,
        kind,
        body,
    }
    .sign(&key(author))
    .unwrap()
}

fn message(author: u8, seq: u64, deps: Vec<RecordId>, auth: &[RecordId]) -> Record {
    channel_record(author, seq, GENERAL, deps, auth.to_vec(), Kind::Message, b"hi".to_vec())
}

fn rotation(author: u8, seq: u64, auth: &[RecordId]) -> Record {
    let mut body = vec![9; 32];
    body.extend_from_slice(&[0, 1]);
    body.extend_from_slice(b"wraps");
    channel_record(author, seq, GENERAL, vec![], auth.to_vec(), Kind::KeyRotation, body)
}

fn cut(heads: &[(ChannelId, RecordId)]) -> Cut {
    Cut::new(heads.to_vec()).unwrap()
}

#[test]
fn members_post_and_others_do_not() {
    let (control, heads) = founded(&[1]);
    let r = resolver(&control);
    let store = Store::default();

    assert_eq!(check_channel(&r, &message(1, 0, vec![], &heads), &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &message(0, 0, vec![], &heads), &store), Verdict::Accept, "owner");
    assert_eq!(
        check_channel(&r, &message(9, 0, vec![], &heads), &store),
        Verdict::Refused(Refusal::NotAMember),
        "a key that never joined"
    );
    assert_eq!(
        check_channel(&r, &message(1, 1, vec![], &[]), &store),
        Verdict::Refused(Refusal::NoAuthDeps)
    );
    let control_post = rec(1, 5, heads.clone(), Action::Post { text: "x".into() });
    assert_eq!(
        check_channel(&r, &control_post, &store),
        Verdict::Refused(Refusal::NotThisServersChannel)
    );
    let empty = channel_record(1, 2, GENERAL, vec![], heads.clone(), Kind::Message, vec![]);
    assert_eq!(check_channel(&r, &empty, &store), Verdict::Accept, "empty text is fine");
}

#[test]
fn a_record_waits_for_its_auth_deps() {
    let (control, _) = founded(&[1]);
    let join = control.last().unwrap().clone();
    let mut r = resolver(&control[..control.len() - 1]);
    let store = Store::default();
    let post = message(1, 0, vec![], &[join.id()]);
    assert_eq!(check_channel(&r, &post, &store), Verdict::Pending { missing: vec![join.id()] });
    r.insert(join);
    assert_eq!(check_channel(&r, &post, &store), Verdict::Accept);
    // A head that never arrives keeps it pending, never accepted.
    let ghost = RecordId::from_bytes([77; 32]);
    let haunted = message(1, 1, vec![], &[ghost]);
    assert_eq!(check_channel(&r, &haunted, &store), Verdict::Pending { missing: vec![ghost] });
}

/// The kicked user signs messages on pre-kick heads. The kick's cut names
/// what the kicker had seen; only that survives.
#[test]
fn a_kick_voids_backdated_messages_by_its_cut() {
    let (mut control, heads) = founded(&[1, 2]);
    let mut store = Store::default();
    let before = message(1, 0, vec![], &heads);
    let reply = message(2, 0, vec![store.add(&before)], &heads);
    let head = store.add(&reply);
    let kick =
        rec(0, 2, heads.clone(), Action::Kick { target: actor(1), cut: cut(&[(GENERAL, head)]) });
    control.push(kick.clone());
    let r = resolver(&control);

    // Seen by the kicker: kept.
    assert_eq!(check_channel(&r, &before, &store), Verdict::Accept);
    // Backdated onto pre-kick heads, after the cut: voided.
    let backdated = message(1, 1, vec![before.id()], &heads);
    assert_eq!(check_channel(&r, &backdated, &store), Verdict::Voided { by: kick.id() });
    // A channel the cut doesn't name isn't voided at all.
    let elsewhere =
        channel_record(1, 2, RANDOM, vec![], heads.clone(), Kind::Message, b"hi".to_vec());
    assert_eq!(check_channel(&r, &elsewhere, &store), Verdict::Accept, "unnamed channel");
    // Signed on heads that include the kick: refused outright.
    let after = message(1, 3, vec![], &[kick.id()]);
    assert_eq!(check_channel(&r, &after, &store), Verdict::Refused(Refusal::NotAMember));
    // Other members are untouched.
    let other = message(2, 1, vec![head], &heads);
    assert_eq!(check_channel(&r, &other, &store), Verdict::Accept);
}

/// A channel with two concurrent heads: the kicker saw both, so the
/// target's message on either branch survives.
#[test]
fn a_record_survives_if_any_head_of_its_channel_saw_it() {
    let (mut control, heads) = founded(&[1, 2, 3]);
    let mut store = Store::default();
    let left = message(1, 0, vec![], &heads);
    let right = message(1, 1, vec![], &heads);
    let (left_id, right_id) = (store.add(&left), store.add(&right));
    let left_head = store.add(&message(2, 0, vec![left_id], &heads));
    let right_head = store.add(&message(3, 0, vec![right_id], &heads));
    let unseen = message(1, 2, vec![left.id(), right.id()], &heads);
    store.add(&unseen);
    let kick = rec(
        0,
        2,
        heads.clone(),
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, left_head), (GENERAL, right_head)]) },
    );
    control.push(kick.clone());
    let r = resolver(&control);

    assert_eq!(check_channel(&r, &left, &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &right, &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &unseen, &store), Verdict::Voided { by: kick.id() });
}

#[test]
fn only_moderators_and_above_rotate() {
    let (mut control, heads) = founded(&[1, 2]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    control.push(moderator.clone());
    let r = resolver(&control);
    let store = Store::default();
    let auth = [moderator.id()];
    assert_eq!(check_channel(&r, &rotation(1, 0, &auth), &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &rotation(0, 0, &auth), &store), Verdict::Accept, "owner");
    assert_eq!(
        check_channel(&r, &rotation(2, 0, &auth), &store),
        Verdict::Refused(Refusal::NotAllowed),
        "member"
    );
    // Judged at its auth_deps: before the grant, 1 was a member.
    assert_eq!(
        check_channel(&r, &rotation(1, 1, &heads), &store),
        Verdict::Refused(Refusal::NotAllowed)
    );
}

/// A revoke voids only the privileged records it hadn't seen: a demoted
/// moderator's backdated rotation, not their messages. After a new grant
/// they rotate again (the revoke doesn't reach records that saw it).
#[test]
fn a_revoke_voids_backdated_rotations_only() {
    let (mut control, heads) = founded(&[1]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let mut store = Store::default();
    let seen_head = store.add(&message(0, 0, vec![], &heads));
    let revoke = rec(
        0,
        3,
        vec![moderator.id()],
        Action::Revoke { target: actor(1), cut: cut(&[(GENERAL, seen_head)]) },
    );
    let regrant =
        rec(0, 4, vec![revoke.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    control.extend([moderator.clone(), revoke.clone(), regrant.clone()]);
    let r = resolver(&control);

    let backdated = rotation(1, 0, &[moderator.id()]);
    assert_eq!(check_channel(&r, &backdated, &store), Verdict::Voided { by: revoke.id() });
    let post = message(1, 0, vec![], &[moderator.id()]);
    assert_eq!(check_channel(&r, &post, &store), Verdict::Accept);
    assert_eq!(
        check_channel(&r, &rotation(1, 1, &[revoke.id()]), &store),
        Verdict::Refused(Refusal::NotAllowed),
        "demoted"
    );
    assert_eq!(check_channel(&r, &rotation(1, 2, &[regrant.id()]), &store), Verdict::Accept);
}

#[test]
fn edits_are_the_authors_and_deletes_need_rank() {
    let (mut control, heads) = founded(&[1, 2, 3]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(2), role: Role::Moderator });
    let admin =
        rec(0, 3, vec![moderator.id()], Action::Grant { target: actor(3), role: Role::Admin });
    control.extend([moderator, admin.clone()]);
    let r = resolver(&control);
    let auth = [admin.id()];
    let mut store = Store::default();
    let by_member = message(1, 0, vec![], &auth);
    let by_admin = message(3, 0, vec![], &auth);
    store.add(&by_member);
    store.add(&by_admin);

    let edit = |author: u8, target: &Record| {
        let body = [target.id().as_bytes().as_slice(), b"fixed"].concat();
        channel_record(
            author,
            9,
            GENERAL,
            vec![target.id()],
            auth.to_vec(),
            Kind::MessageEdit,
            body,
        )
    };
    let delete = |author: u8, target: &Record| {
        let body = target.id().as_bytes().to_vec();
        channel_record(
            author,
            10,
            GENERAL,
            vec![target.id()],
            auth.to_vec(),
            Kind::MessageDelete,
            body,
        )
    };
    let refused = Verdict::Refused(Refusal::NotAllowed);
    assert_eq!(check_channel(&r, &edit(1, &by_member), &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &edit(2, &by_member), &store), refused, "a moderator can't edit");
    assert_eq!(check_channel(&r, &delete(1, &by_member), &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &delete(2, &by_member), &store), Verdict::Accept, "mod > member");
    assert_eq!(check_channel(&r, &delete(2, &by_admin), &store), refused, "mod < admin");
    assert_eq!(check_channel(&r, &delete(1, &by_admin), &store), refused, "member");

    // The target must be in deps, and a delete's body is exactly its ID.
    let loose = channel_record(
        1,
        11,
        GENERAL,
        vec![],
        auth.to_vec(),
        Kind::MessageDelete,
        by_member.id().as_bytes().to_vec(),
    );
    assert_eq!(check_channel(&r, &loose, &store), Verdict::Refused(Refusal::BadTarget));
    let long = channel_record(
        1,
        12,
        GENERAL,
        vec![by_member.id()],
        auth.to_vec(),
        Kind::MessageDelete,
        [by_member.id().as_bytes().as_slice(), b"x"].concat(),
    );
    assert_eq!(check_channel(&r, &long, &store), Verdict::Refused(Refusal::BadTarget));
    // A target the store doesn't hold yet: pending.
    let unheld = message(1, 13, vec![], &auth);
    assert_eq!(
        check_channel(&r, &edit(1, &unheld), &store),
        Verdict::Pending { missing: vec![unheld.id()] }
    );
}

#[test]
fn rule_b_hides_instead_of_voiding_messages() {
    let (mut control, heads) = founded(&[1]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let kick =
        rec(0, 3, vec![moderator.id()], Action::Kick { target: actor(1), cut: Cut::default() });
    control.extend([moderator.clone(), kick.clone()]);
    let r = resolver(&control);
    let store = Store::default();
    let auth = [moderator.id()];
    let post = message(1, 0, vec![], &auth);
    let rotate = rotation(1, 0, &auth);
    assert_eq!(
        check_channel_with(&r, &post, &store, privileged_only),
        Verdict::Hidden { by: kick.id() }
    );
    assert_eq!(
        check_channel_with(&r, &rotate, &store, privileged_only),
        Verdict::Voided { by: kick.id() }
    );
    // Rule A with an empty cut: nothing named, nothing voided.
    assert_eq!(check_channel(&r, &post, &store), Verdict::Accept);
}

/// Verdicts depend only on the control records held, not their order.
#[test]
fn verdicts_do_not_depend_on_control_delivery_order() {
    let (mut control, heads) = founded(&[1, 2]);
    let mut store = Store::default();
    let seen = message(1, 0, vec![], &heads);
    let seen_id = store.add(&seen);
    let head = store.add(&message(2, 0, vec![seen_id], &heads));
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let kick = rec(
        0,
        3,
        vec![moderator.id()],
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, head)]) },
    );
    control.extend([moderator.clone(), kick]);
    let channel = [
        seen.clone(),
        message(1, 1, vec![seen.id()], &heads),
        rotation(1, 0, &[moderator.id()]),
        message(2, 1, vec![head], &[moderator.id()]),
        message(9, 0, vec![], &heads),
    ];
    let expected: Vec<Verdict> =
        channel.iter().map(|record| check_channel(&resolver(&control), record, &store)).collect();
    assert!(expected.contains(&Verdict::Accept));
    assert!(expected.iter().any(|v| matches!(v, Verdict::Voided { .. })));
    let mut rng = StdRng::seed_from_u64(3);
    for _ in 0..50 {
        control.shuffle(&mut rng);
        let r = resolver(&control);
        let got: Vec<Verdict> =
            channel.iter().map(|record| check_channel(&r, record, &store)).collect();
        assert_eq!(got, expected);
    }
}

#[test]
fn message_text_is_limited_in_code_points() {
    let emoji = "😀".repeat(MAX_MESSAGE_CODE_POINTS);
    assert!(message_text_ok(&emoji), "4,000 code points of 4 bytes each");
    assert!(!message_text_ok(&format!("{emoji}a")));
    assert!(message_text_ok(&"a".repeat(MAX_MESSAGE_CODE_POINTS)));
    assert!(message_text_ok(""));
}
