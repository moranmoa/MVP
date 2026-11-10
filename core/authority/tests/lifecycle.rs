//! Server lifecycle (docs/design/server-rename-leave-delete.md, piece 1):
//! rename (`ServerEdit`), leave (`Leave`) and delete (`ServerClose`). Every
//! test runs the reference `resolve` and the incremental `Resolver` over
//! many delivery orders and requires them to agree.

mod common;
mod rng;

use authority::{
    check_channel, resolve, Action, ChannelGraph, ChannelId, Record, RecordId, Refusal, Resolver,
    Role, State, Verdict, MAX_SERVER_NAME,
};
use common::{actor, founded, key, rec, server};
use records::{Draft, Kind, Scope};
use rng::{SliceRandom, StdRng};

const ORDERS: u64 = 200;

fn all_orders(records: &[Record]) -> State {
    let expected = resolve(server(), records);
    let mut rng = StdRng::seed_from_u64(29);
    let mut arriving = records.to_vec();
    for round in 0..=ORDERS {
        if round > 0 {
            arriving.shuffle(&mut rng);
        }
        assert_eq!(resolve(server(), &arriving), expected, "resolve depends on order");
        let mut resolver = Resolver::new(server());
        for record in &arriving {
            resolver.insert(record.clone());
        }
        assert_eq!(resolver.state(), &expected, "incremental disagrees with resolve");
    }
    expected
}

fn rename(author: u8, seq: u64, deps: Vec<RecordId>, name: &str) -> Record {
    rec(author, seq, deps, Action::ServerEdit { name: name.into() })
}

fn leave(author: u8, seq: u64, deps: Vec<RecordId>) -> Record {
    rec(author, seq, deps, Action::Leave { cut: Default::default() })
}

fn close(seq: u64, deps: Vec<RecordId>) -> Record {
    rec(0, seq, deps, Action::ServerClose)
}

fn raw(author: u8, seq: u64, deps: Vec<RecordId>, kind: Kind, body: Vec<u8>) -> Record {
    Draft {
        server: server(),
        scope: Scope::Control,
        seq,
        deps,
        auth_deps: vec![],
        ts: 0,
        kind,
        body,
    }
    .sign(&key(author))
    .unwrap()
}

fn invite_and_join(owner_seq: u64, deps: Vec<RecordId>, joiner: u8, seq: u64) -> [Record; 2] {
    let link = rec(0, owner_seq, deps, Action::Invite { expires_ms: 0, max_uses: 0 });
    let join = rec(joiner, seq, vec![link.id()], Action::Join { invite: link.id() });
    [link, join]
}

#[test]
fn the_owner_or_an_admin_renames_and_the_latest_counts() {
    let (mut records, heads) = founded(&[1, 2, 3]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let moderator =
        rec(0, 3, vec![admin.id()], Action::Grant { target: actor(2), role: Role::Moderator });
    let by_owner = rename(0, 4, vec![moderator.id()], "One");
    let by_admin = rename(1, 1, vec![by_owner.id()], "Two");
    let by_moderator = rename(2, 1, vec![by_admin.id()], "Three");
    let by_member = rename(3, 1, vec![by_admin.id()], "Four");
    records.extend([admin, moderator, by_owner, by_admin, by_moderator.clone(), by_member.clone()]);
    let state = all_orders(&records);
    assert_eq!(state.server_name.as_deref(), Some("Two"));
    assert!(!state.accepted.contains(&by_moderator.id()));
    assert!(!state.accepted.contains(&by_member.id()));

    // Two renames that don't see each other: linear order decides, never `ts`.
    let (mut records, heads) = founded(&[]);
    let a = rename(0, 2, heads.clone(), "Alpha");
    let b = rename(0, 3, heads, "Beta");
    records.extend([a.clone(), b.clone()]);
    let state = all_orders(&records);
    let later = if a.id() > b.id() { "Alpha" } else { "Beta" };
    assert_eq!(state.server_name.as_deref(), Some(later));
}

/// The body is `0u8 ‖ JSON {v:1, name}`: unknown fields are ignored, `v` must
/// be 1, and the name keeps the strict rule (never the display-name one).
#[test]
fn rename_bodies_are_forward_compatible_and_strict() {
    let (mut records, heads) = founded(&[]);
    let body = |json: &str| [vec![0], json.as_bytes().to_vec()].concat();
    let later_version = raw(
        0,
        2,
        heads.clone(),
        Kind::ServerEdit,
        body(r#"{"v":1,"name":"Next","icon":"x","description":{"a":1}}"#),
    );
    assert_eq!(
        Action::from_record(&later_version),
        Some(Action::ServerEdit { name: "Next".into() })
    );

    let long = "a".repeat(MAX_SERVER_NAME + 1);
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let bad_jsons = [
        r#"{"v":2,"name":"x"}"#.to_owned(),
        r#"{"name":"x"}"#.to_owned(),
        r#"{"v":1}"#.to_owned(),
        r#"{"v":1,"name":7}"#.to_owned(),
        r#"{"v":1,"name":""}"#.to_owned(),
        r#"{"v":1,"name":" padded"}"#.to_owned(),
        format!(r#"{{"v":1,"name":"{long}"}}"#),
        // A JSON escape for U+202E (RIGHT-TO-LEFT OVERRIDE).
        String::from("{\"v\":1,\"name\":\"a") + "\\" + "u202Eb\"}",
        format!(r#"{{"v":1,"name":"{family}"}}"#),
        "not json".to_owned(),
    ];
    let mut refused = Vec::new();
    for (seq, json) in bad_jsons.iter().enumerate() {
        let record = raw(0, 10 + seq as u64, heads.clone(), Kind::ServerEdit, body(json));
        assert_eq!(Action::from_record(&record), None, "{json}");
        refused.push(record);
    }
    let history =
        raw(0, 30, heads, Kind::ServerEdit, [vec![1], br#"{"v":1,"name":"x"}"#.to_vec()].concat());
    assert_eq!(Action::from_record(&history), None, "history byte set");
    let exactly_long = "a".repeat(MAX_SERVER_NAME);
    assert!(Action::from_record(&rename(0, 31, vec![], &exactly_long)).is_some());

    records.push(later_version);
    records.extend(refused);
    records.push(history);
    let state = all_orders(&records);
    assert_eq!(state.server_name.as_deref(), Some("Next"));
}

/// An admin's rename signed on heads from before their demotion is voided,
/// like any of their privileged records.
#[test]
fn a_backdated_admin_rename_is_voided_by_the_revoke() {
    let (mut records, heads) = founded(&[1]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let renamed = rename(1, 1, vec![admin.id()], "Mine now");
    let revoke = rec(0, 3, vec![admin.id()], Action::revoke(actor(1)));
    records.extend([admin, renamed.clone(), revoke]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&renamed.id()));
    assert_eq!(state.server_name, None);
}

/// A leave ends membership and any role; a rejoin starts as a member.
#[test]
fn a_leave_ends_the_role_and_a_rejoin_starts_over() {
    let (mut records, heads) = founded(&[1, 2]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let gone = leave(1, 1, vec![admin.id()]);
    let after = rec(1, 2, vec![gone.id()], Action::Post { text: "still here?".into() });
    let owner_leaves = leave(0, 3, vec![admin.id()]);
    records.extend([admin, gone.clone(), after.clone(), owner_leaves.clone()]);
    let state = all_orders(&records);
    assert!(!state.is_member(actor(1)));
    assert_eq!(state.role(actor(1)), Role::Member);
    assert!(!state.kicked.contains(&actor(1)), "a leaver isn't banned");
    assert!(!state.accepted.contains(&after.id()), "a leaver can't post");
    assert!(!state.accepted.contains(&owner_leaves.id()), "the owner can't leave");
    assert!(state.is_member(actor(0)) && state.is_member(actor(2)));

    let [link, rejoin] = invite_and_join(4, vec![gone.id()], 1, 3);
    records.extend([link, rejoin.clone()]);
    let state = all_orders(&records);
    assert!(state.accepted.contains(&rejoin.id()));
    assert!(state.is_member(actor(1)));
    assert_eq!(state.role(actor(1)), Role::Member, "no admin back");
}

/// A kicked key can't leave, so its ban survives, and a rejoin stays refused.
#[test]
fn a_leave_cant_erase_a_kick() {
    let (mut records, heads) = founded(&[1]);
    let kick = rec(0, 2, heads, Action::kick(actor(1)));
    let gone = leave(1, 1, vec![kick.id()]);
    let [link, rejoin] = invite_and_join(3, vec![gone.id()], 1, 2);
    records.extend([kick.clone(), gone.clone(), link, rejoin.clone()]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&gone.id()));
    assert!(state.kicked.contains(&actor(1)));
    assert_eq!(state.removed.get(&actor(1)), Some(&kick.id()));
    assert!(!state.accepted.contains(&rejoin.id()));
}

/// A leave and a grant to the same person that don't see each other are
/// settled by linear order, the same in every delivery order: either way the
/// leaver ends up out, with no role.
#[test]
fn a_leave_concurrent_with_a_grant_is_settled_by_order() {
    let (mut records, heads) = founded(&[1]);
    let gone = leave(1, 1, heads.clone());
    let grant = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    records.extend([gone.clone(), grant.clone()]);
    let state = all_orders(&records);
    assert!(!state.is_member(actor(1)));
    assert_eq!(state.role(actor(1)), Role::Member);
    assert!(state.accepted.contains(&gone.id()));
    assert_eq!(state.accepted.contains(&grant.id()), grant.id() < gone.id());
}

/// Security-lead rule 1: after a close, only its causal past stays accepted.
/// A post and a grant concurrent with it are refused on every arrival order,
/// even when they arrived and were accepted before the close.
#[test]
fn a_close_keeps_only_its_causal_past() {
    let (mut records, heads) = founded(&[1, 2]);
    let kept = rec(1, 1, heads.clone(), Action::Post { text: "before".into() });
    let concurrent_post = rec(2, 1, heads.clone(), Action::Post { text: "racing".into() });
    let concurrent_grant =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Admin });
    let closed = close(3, vec![kept.id()]);
    let after = rec(0, 4, vec![closed.id()], Action::Post { text: "after".into() });
    records.extend([
        kept.clone(),
        concurrent_post.clone(),
        concurrent_grant.clone(),
        closed.clone(),
        after.clone(),
    ]);
    let state = all_orders(&records);
    assert_eq!(state.closed, Some(closed.id()));
    assert!(state.accepted.contains(&kept.id()) && state.accepted.contains(&closed.id()));
    for refused in [&concurrent_post, &concurrent_grant, &after] {
        assert!(!state.accepted.contains(&refused.id()));
    }
    assert_eq!(state.role(actor(1)), Role::Member);
    assert_eq!(state.posts, vec![kept.id()]);
}

/// A member who signs on pre-close heads after the close (backdating) is
/// refused, wherever the record sorts in linear order.
#[test]
fn a_backdated_record_after_a_close_is_refused() {
    let (mut records, heads) = founded(&[1]);
    let closed = close(2, heads.clone());
    let backdated: Vec<Record> = (1..20)
        .map(|seq| rec(1, seq, heads.clone(), Action::Post { text: format!("late {seq}") }))
        .collect();
    assert!(backdated.iter().any(|r| r.id() < closed.id()));
    assert!(backdated.iter().any(|r| r.id() > closed.id()));
    records.push(closed.clone());
    records.extend(backdated.iter().cloned());
    let state = all_orders(&records);
    assert_eq!(state.closed, Some(closed.id()));
    assert!(backdated.iter().all(|r| !state.accepted.contains(&r.id())));
}

/// Only the owner closes, and the first close in linear order wins.
#[test]
fn only_the_owner_closes_and_the_first_close_wins() {
    let (mut records, heads) = founded(&[1]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let by_admin = rec(1, 1, vec![admin.id()], Action::ServerClose);
    records.extend([admin.clone(), by_admin.clone()]);
    let state = all_orders(&records);
    assert_eq!(state.closed, None, "an admin can't delete the server");

    let first = close(3, vec![admin.id()]);
    let second = close(4, vec![admin.id()]);
    let after_both = close(5, vec![first.id(), second.id()]);
    records.extend([first.clone(), second.clone(), after_both.clone()]);
    let state = all_orders(&records);
    let winner = first.id().min(second.id());
    assert_eq!(state.closed, Some(winner));
    for other in [first.id(), second.id(), after_both.id()].into_iter().filter(|id| *id != winner) {
        assert!(!state.accepted.contains(&other));
    }
    assert!(!state.accepted.contains(&by_admin.id()), "concurrent with the close");

    // A body other than empty isn't a close.
    let padded = raw(0, 9, vec![admin.id()], Kind::ServerClose, vec![0]);
    assert_eq!(Action::from_record(&padded), None);
}

struct NoChannels;

impl ChannelGraph for NoChannels {
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool {
        ancestor == of
    }

    fn record(&self, _: RecordId) -> Option<Record> {
        None
    }

    fn children(&self, _: RecordId) -> Vec<RecordId> {
        Vec::new()
    }
}

#[test]
fn a_closed_server_takes_no_channel_records() {
    let (records, heads) = founded(&[1]);
    let message = |seq, auth: &[RecordId]| {
        Draft {
            server: server(),
            scope: Scope::Channel(ChannelId::from_bytes([5; 32])),
            seq,
            deps: vec![],
            auth_deps: auth.to_vec(),
            ts: 0,
            kind: Kind::Message,
            body: b"hi".to_vec(),
        }
        .sign(&key(1))
        .unwrap()
    };
    let mut resolver = Resolver::new(server());
    for record in &records {
        resolver.insert(record.clone());
    }
    assert_eq!(check_channel(&resolver, &message(0, &heads), &NoChannels), Verdict::Accept);
    resolver.insert(close(2, heads.clone()));
    assert_eq!(
        check_channel(&resolver, &message(1, &heads), &NoChannels),
        Verdict::Refused(Refusal::ServerClosed)
    );
}
