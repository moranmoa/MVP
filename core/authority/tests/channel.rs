//! Channel records judged against the control log (GAP-2, GAP-5): who may
//! post, edit, delete and rotate, pending `auth_deps`, and voiding by the
//! cut of a kick or revoke.

mod common;

use std::collections::HashMap;

use authority::{
    check_channel, check_channel_with, check_stub, message_text_ok, privileged_only, Action,
    ChannelGraph, ChannelId, Cut, Entry, Record, RecordId, Refusal, Resolver, Role, Stub, Verdict,
    MAX_MESSAGE_CODE_POINTS,
};
use common::{actor, founded, key, rec, server};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use records::{Draft, Kind, Scope};

const GENERAL: ChannelId = ChannelId::from_bytes([5; 32]);
const RANDOM: ChannelId = ChannelId::from_bytes([6; 32]);

/// The caller's channel store: records (in full or pruned) by ID, ancestry
/// through deps.
#[derive(Default)]
struct Store(HashMap<RecordId, Entry>);

impl Store {
    fn add(&mut self, record: &Record) -> RecordId {
        self.0.insert(record.id(), Entry::Full(record.clone()));
        record.id()
    }

    /// Keeps only the stub of a held record.
    fn prune(&mut self, record: &Record) -> Stub {
        let stub = record.prune().expect("prunable");
        self.0.insert(stub.id(), Entry::Pruned(stub.clone()));
        stub
    }
}

impl ChannelGraph for Store {
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool {
        let mut stack = vec![of];
        while let Some(next) = stack.pop() {
            if next == ancestor {
                return true;
            }
            if let Some(entry) = self.0.get(&next) {
                stack.extend_from_slice(entry.deps());
            }
        }
        false
    }

    fn record(&self, id: RecordId) -> Option<Record> {
        match self.0.get(&id)? {
            Entry::Full(record) => Some(record.clone()),
            Entry::Pruned(_) => None,
        }
    }

    fn entry(&self, id: RecordId) -> Option<Entry> {
        self.0.get(&id).cloned()
    }

    fn children(&self, id: RecordId) -> Vec<RecordId> {
        self.0.values().filter(|e| e.deps().contains(&id)).map(Entry::id).collect()
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

/// A kick signed on a stale cut: the kicker hadn't synced the replies.
/// What another member already replied to survives; the rest is voided.
#[test]
fn a_stale_cut_keeps_what_others_witnessed() {
    let (mut control, heads) = founded(&[1, 2, 3]);
    let mut store = Store::default();
    let old_head = store.add(&message(2, 0, vec![], &heads));
    // After the kicker's view: a message of the target that member 3 replied
    // to, one the target replied to themself, and one nobody saw.
    let replied = message(1, 0, vec![old_head], &heads);
    store.add(&replied);
    store.add(&message(3, 0, vec![replied.id()], &heads));
    let self_replied = message(1, 1, vec![old_head], &heads);
    store.add(&self_replied);
    store.add(&message(1, 2, vec![self_replied.id()], &heads));
    let unseen = message(1, 3, vec![old_head], &heads);
    store.add(&unseen);
    // Seen only by a record that isn't accepted (a key that never joined).
    let shadow = message(1, 4, vec![old_head], &heads);
    store.add(&shadow);
    store.add(&message(9, 0, vec![shadow.id()], &heads));
    // A privileged record keeps the plain cut rule, witnessed or not.
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let rotate = rotation(1, 0, &[moderator.id()]);
    store.add(&rotate);
    store.add(&message(3, 1, vec![rotate.id()], &heads));
    let kick = rec(
        0,
        3,
        vec![moderator.id()],
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, old_head)]) },
    );
    control.extend([moderator, kick.clone()]);
    // A "witness" signed after the kick was known doesn't count: a colluder
    // can't revive a backdated message.
    let late = message(1, 5, vec![old_head], &heads);
    store.add(&late);
    store.add(&message(3, 2, vec![late.id()], &[kick.id()]));
    // A witness further down, through the target's own reply, still counts.
    let deep = message(1, 6, vec![old_head], &heads);
    let own = message(1, 7, vec![deep.id()], &heads);
    store.add(&deep);
    store.add(&own);
    store.add(&message(2, 2, vec![own.id()], &heads));
    let r = resolver(&control);

    let voided = Verdict::Voided { by: kick.id() };
    assert_eq!(check_channel(&r, &replied, &store), Verdict::Accept, "witnessed by member 3");
    assert_eq!(check_channel(&r, &self_replied, &store), voided, "the target can't witness itself");
    assert_eq!(check_channel(&r, &unseen, &store), voided, "nobody saw it");
    assert_eq!(check_channel(&r, &shadow, &store), voided, "the witness isn't accepted");
    assert_eq!(check_channel(&r, &late, &store), voided, "witnessed only after the kick");
    assert_eq!(check_channel(&r, &deep, &store), Verdict::Accept, "witnessed further down");
    assert_eq!(check_channel(&r, &rotate, &store), voided, "privileged: the cut alone decides");
}

/// A cut head the caller doesn't hold yet: the record may well be an
/// ancestor of it, so the verdict waits instead of flickering to void.
#[test]
fn a_missing_cut_head_is_pending_not_void() {
    let (mut control, heads) = founded(&[1, 2]);
    let mut store = Store::default();
    let mine = message(1, 0, vec![], &heads);
    store.add(&mine);
    let reply = message(2, 0, vec![mine.id()], &heads);
    let kick = rec(
        0,
        2,
        heads.clone(),
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, reply.id())]) },
    );
    control.push(kick);
    let r = resolver(&control);
    assert_eq!(check_channel(&r, &mine, &store), Verdict::Pending { missing: vec![reply.id()] });
    store.add(&reply);
    assert_eq!(check_channel(&r, &mine, &store), Verdict::Accept);
}

/// Edits and deletes change messages only, never a rotation or another
/// edit or delete.
#[test]
fn only_messages_are_edited_or_deleted() {
    let (mut control, heads) = founded(&[1, 2]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    control.push(moderator.clone());
    let r = resolver(&control);
    let auth = vec![moderator.id()];
    let mut store = Store::default();
    let rotate = rotation(1, 0, &auth);
    let note = message(1, 1, vec![], &auth);
    store.add(&rotate);
    store.add(&note);
    let fix = channel_record(
        1,
        2,
        GENERAL,
        vec![note.id()],
        auth.clone(),
        Kind::MessageEdit,
        [note.id().as_bytes().as_slice(), b"x"].concat(),
    );
    store.add(&fix);
    let other_channel =
        channel_record(1, 3, RANDOM, vec![], auth.clone(), Kind::Message, b"hi".to_vec());
    store.add(&other_channel);

    let edit = |target: &Record, seq: u64| {
        let body = [target.id().as_bytes().as_slice(), b"new"].concat();
        channel_record(1, seq, GENERAL, vec![target.id()], auth.clone(), Kind::MessageEdit, body)
    };
    let delete = |author: u8, target: &Record, seq: u64| {
        let body = target.id().as_bytes().to_vec();
        channel_record(
            author,
            seq,
            GENERAL,
            vec![target.id()],
            auth.clone(),
            Kind::MessageDelete,
            body,
        )
    };
    let bad = Verdict::Refused(Refusal::BadTarget);
    assert_eq!(check_channel(&r, &edit(&note, 10), &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &edit(&rotate, 11), &store), bad, "edit a rotation");
    assert_eq!(check_channel(&r, &edit(&fix, 12), &store), bad, "edit an edit");
    assert_eq!(check_channel(&r, &edit(&other_channel, 13), &store), bad, "another channel");
    assert_eq!(check_channel(&r, &delete(1, &rotate, 14), &store), bad, "delete own rotation");
    assert_eq!(check_channel(&r, &delete(0, &rotate, 0), &store), bad, "owner deletes a rotation");
    assert_eq!(check_channel(&r, &delete(1, &fix, 15), &store), bad, "delete an edit");
}

/// A kicker can't freeze a channel by naming a head that doesn't exist:
/// what a held head or a witness keeps is still kept; only a record nothing
/// keeps waits on the missing head.
#[test]
fn a_fake_cut_head_holds_back_nothing_already_kept() {
    let (mut control, heads) = founded(&[1, 2]);
    let mut store = Store::default();
    let seen = message(1, 0, vec![], &heads);
    let real_head = message(2, 0, vec![seen.id()], &heads);
    let replied = message(1, 1, vec![], &heads);
    let reply = message(2, 1, vec![replied.id()], &heads);
    let alone = message(1, 2, vec![], &heads);
    for record in [&seen, &real_head, &replied, &reply, &alone] {
        store.add(record);
    }
    let fake = RecordId::from_bytes([66; 32]);
    let kick = rec(
        0,
        2,
        heads.clone(),
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, real_head.id()), (GENERAL, fake)]) },
    );
    control.push(kick);
    let r = resolver(&control);
    assert_eq!(check_channel(&r, &seen, &store), Verdict::Accept, "ancestor of a held head");
    assert_eq!(check_channel(&r, &replied, &store), Verdict::Accept, "witnessed");
    assert_eq!(check_channel(&r, &alone, &store), Verdict::Pending { missing: vec![fake] });
}

fn edit(author: u8, seq: u64, target: &Record, deps: Vec<RecordId>, auth: &[RecordId]) -> Record {
    let body = [target.id().as_bytes().as_slice(), b"new text"].concat();
    channel_record(author, seq, GENERAL, deps, auth.to_vec(), Kind::MessageEdit, body)
}

fn delete(author: u8, seq: u64, target: &Record, auth: &[RecordId]) -> Record {
    let body = target.id().as_bytes().to_vec();
    channel_record(
        author,
        seq,
        GENERAL,
        vec![target.id()],
        auth.to_vec(),
        Kind::MessageDelete,
        body,
    )
}

/// B6-p: a deleted message is kept as a stub. A new member syncing it gets
/// the same verdict as for the full record, and the delete naming it isn't
/// left waiting for a body that's gone.
#[test]
fn a_deleted_message_prunes_to_a_stub_that_still_counts() {
    let (control, heads) = founded(&[1, 2]);
    let r = resolver(&control);
    let mut store = Store::default();
    let message = message(1, 0, vec![], &heads);
    let gone = delete(1, 1, &message, &heads);
    store.add(&message);
    store.add(&gone);
    let stub = store.prune(&message);
    assert_eq!(check_stub(&r, &stub, &store), Verdict::Accept);
    assert_eq!(check_channel(&r, &gone, &store), Verdict::Accept, "the target is held as a stub");
    // A reply to the deleted message doesn't wait on it either.
    let reply = self::message(2, 0, vec![message.id()], &heads);
    assert_eq!(check_channel(&r, &reply, &store), Verdict::Accept);
}

/// A stub stands in only once an accepted delete or edit replaced it.
/// Otherwise the verdict is pending on the stub's own ID: fetch the full
/// record. A peer serving stubs can't make messages disappear.
#[test]
fn a_stub_without_an_accepted_replacement_is_pending() {
    let (control, heads) = founded(&[1, 2]);
    let r = resolver(&control);
    let mut store = Store::default();
    let message = message(1, 0, vec![], &heads);
    let stub = store.prune(&message);
    let fetch = Verdict::Pending { missing: vec![message.id()] };
    assert_eq!(check_stub(&r, &stub, &store), fetch, "nothing replaced it");
    // A delete that isn't accepted (a member deleting another's) doesn't count.
    store.add(&delete(2, 0, &message, &heads));
    assert_eq!(check_stub(&r, &stub, &store), fetch, "the delete is refused");
    store.add(&delete(1, 1, &message, &heads));
    assert_eq!(check_stub(&r, &stub, &store), Verdict::Accept);
}

/// An edit replaces the message's text, and a newer edit an older edit;
/// a delete replaces both. Edits are pruned on the same terms.
#[test]
fn edits_replace_text_and_prune_too() {
    let (control, heads) = founded(&[1]);
    let r = resolver(&control);
    let mut store = Store::default();
    let message = message(1, 0, vec![], &heads);
    let first = edit(1, 1, &message, vec![message.id()], &heads);
    store.add(&message);
    store.add(&first);
    let message_stub = store.prune(&message);
    assert_eq!(check_stub(&r, &message_stub, &store), Verdict::Accept, "replaced by the edit");

    let first_stub = store.prune(&first);
    let fetch = Verdict::Pending { missing: vec![first.id()] };
    assert_eq!(check_stub(&r, &first_stub, &store), fetch, "the latest edit is still the text");
    let second = edit(1, 2, &message, vec![message.id(), first.id()], &heads);
    store.add(&second);
    assert_eq!(check_stub(&r, &first_stub, &store), Verdict::Accept, "a newer edit replaced it");

    // An older edit doesn't replace a newer one it doesn't descend from.
    let second_stub = store.prune(&second);
    assert_eq!(
        check_stub(&r, &second_stub, &store),
        Verdict::Pending { missing: vec![second.id()] }
    );
    store.add(&delete(1, 3, &message, &heads));
    assert_eq!(check_stub(&r, &second_stub, &store), Verdict::Accept, "the delete replaced all");
}

/// A stub is judged like its record (a kicked author's backdated message
/// stays voided). Pruning a reply doesn't take away its witness: the delete
/// that replaced it descends from the message too and witnesses it.
#[test]
fn a_stub_is_judged_the_same_and_pruning_keeps_witnesses() {
    let (mut control, heads) = founded(&[1, 2, 3]);
    let mut store = Store::default();
    let old_head = store.add(&message(3, 0, vec![], &heads));
    let target = message(1, 0, vec![old_head], &heads);
    let reply = message(2, 0, vec![target.id()], &heads);
    let alone = message(1, 1, vec![old_head], &heads);
    store.add(&target);
    store.add(&reply);
    store.add(&alone);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(3), role: Role::Moderator });
    let kick = rec(
        0,
        3,
        vec![moderator.id()],
        Action::Kick { target: actor(1), cut: cut(&[(GENERAL, old_head)]) },
    );
    control.extend([moderator.clone(), kick.clone()]);
    let r = resolver(&control);
    assert_eq!(check_channel(&r, &target, &store), Verdict::Accept, "witnessed by the reply");

    // 2 deletes their reply and it is pruned: the delete still witnesses.
    store.add(&delete(2, 1, &reply, &heads));
    store.prune(&reply);
    assert_eq!(check_channel(&r, &target, &store), Verdict::Accept, "the delete saw it too");
    // The kicked author's unwitnessed message, deleted by the moderator after
    // the kick (a delete that knows the kick isn't a witness) and pruned:
    // judged like the record, voided.
    assert_eq!(check_channel(&r, &alone, &store), Verdict::Voided { by: kick.id() });
    let cleanup = delete(3, 1, &alone, &[kick.id()]);
    assert_eq!(check_channel(&r, &cleanup, &store), Verdict::Accept);
    store.add(&cleanup);
    let full = check_channel(&r, &alone, &store);
    let stub = store.prune(&alone);
    assert_eq!(check_stub(&r, &stub, &store), full);
    assert_eq!(full, Verdict::Voided { by: kick.id() });
}

#[test]
fn stubs_round_trip_through_the_store_format() {
    let (_, heads) = founded(&[1]);
    let message = message(1, 0, vec![], &heads);
    let stub: Stub = message.prune().unwrap();
    assert_eq!(Stub::decode(&stub.encode()).unwrap(), stub);
    assert_eq!(Entry::Pruned(stub.clone()).id(), message.id());
    assert!(rotation(1, 9, &heads).prune().is_none(), "rotations aren't pruned");
}
