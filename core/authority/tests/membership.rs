//! Membership (GAP-3): invites, joins, revoked invites, and who counts as a
//! member. Every test runs the reference `resolve` and the incremental
//! `Resolver` over many delivery orders and requires them to agree.

mod common;

use authority::{resolve, Action, Record, RecordId, Resolver, Role, State};
use common::{actor, founded, genesis, key, rec, rec_ts, server};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use records::{Draft, Kind, Scope};

const ORDERS: u64 = 200;

/// Resolves `records` with both resolvers, in the given order and in
/// `ORDERS` random ones, and returns the one state they all agree on.
fn all_orders(records: &[Record]) -> State {
    let expected = resolve(server(), records);
    let mut rng = StdRng::seed_from_u64(7);
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

fn invite(author: u8, seq: u64, deps: Vec<RecordId>, expires_ms: u64, max_uses: u32) -> Record {
    rec(author, seq, deps, Action::Invite { expires_ms, max_uses })
}

fn join(author: u8, seq: u64, invite: &Record) -> Record {
    rec(author, seq, vec![invite.id()], Action::Join { invite: invite.id() })
}

#[test]
fn members_are_the_owner_and_accepted_joins() {
    let (records, _) = founded(&[1, 2]);
    let state = all_orders(&records);
    assert_eq!(state.members, [actor(0), actor(1), actor(2)].into());
    assert!(!state.is_member(actor(3)));
    let invite = state.invites[&records[1].id()];
    assert_eq!((invite.author, invite.uses, invite.revoked), (actor(0), 2, false));
}

#[test]
fn a_non_member_key_can_neither_post_nor_be_granted() {
    let (mut records, heads) = founded(&[1]);
    let outsider = 9;
    let post = rec(outsider, 0, heads.clone(), Action::Post { text: "spam".into() });
    let grant =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(outsider), role: Role::Admin });
    let member_post = rec(1, 1, heads, Action::Post { text: "hello".into() });
    records.extend([post.clone(), grant.clone(), member_post.clone()]);

    let state = all_orders(&records);
    assert!(!state.accepted.contains(&post.id()));
    assert!(!state.accepted.contains(&grant.id()));
    assert_eq!(state.posts, vec![member_post.id()]);
    assert_eq!(state.role(actor(outsider)), Role::Member);
}

#[test]
fn only_moderators_and_above_sign_invites() {
    let (mut records, heads) = founded(&[1, 2, 3]);
    let admin = rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Admin });
    let moderator =
        rec(0, 3, vec![admin.id()], Action::Grant { target: actor(2), role: Role::Moderator });
    let by_admin = invite(1, 1, vec![moderator.id()], 0, 0);
    let by_moderator = invite(2, 1, vec![moderator.id()], 0, 0);
    let by_member = invite(3, 1, vec![moderator.id()], 0, 0);
    let by_outsider = invite(9, 0, vec![moderator.id()], 0, 0);
    let joins = [join(4, 0, &by_admin), join(5, 0, &by_moderator), join(6, 0, &by_member)];
    let via_outsider = join(7, 0, &by_outsider);
    records.extend([admin, moderator, by_admin.clone(), by_moderator.clone()]);
    records.extend([by_member.clone(), by_outsider.clone(), via_outsider]);
    records.extend(joins);

    let state = all_orders(&records);
    assert!(state.invites.contains_key(&by_admin.id()));
    assert!(state.invites.contains_key(&by_moderator.id()));
    assert!(!state.invites.contains_key(&by_member.id()));
    assert!(!state.invites.contains_key(&by_outsider.id()));
    assert!(state.is_member(actor(4)) && state.is_member(actor(5)));
    assert!(!state.is_member(actor(6)) && !state.is_member(actor(7)));
}

#[test]
fn a_join_must_name_an_invite_it_depends_on() {
    let g = genesis();
    let invite = invite(0, 1, vec![g.id()], 0, 0);
    let post = rec(0, 2, vec![invite.id()], Action::Post { text: "not an invite".into() });
    // Names the invite but builds on something else.
    let detached = rec(1, 0, vec![post.id()], Action::Join { invite: invite.id() });
    // Names a record that isn't an invite.
    let via_post = rec(2, 0, vec![post.id()], Action::Join { invite: post.id() });
    // Names an invite of another server: not even in the log.
    let unknown = rec(
        3,
        0,
        vec![post.id(), RecordId::from_bytes([5; 32])],
        Action::Join { invite: RecordId::from_bytes([5; 32]) },
    );
    let ok = join(4, 0, &invite);

    let state = all_orders(&[g, invite, post, detached, via_post, unknown, ok]);
    assert_eq!(state.members, [actor(0), actor(4)].into());
}

#[test]
fn use_limit_counts_joins_in_linear_order() {
    let g = genesis();
    let twice = invite(0, 1, vec![g.id()], 0, 2);
    let joins: Vec<Record> = (1..=4).map(|n| join(n, 0, &twice)).collect();
    let mut records = vec![g, twice.clone()];
    records.extend(joins.iter().cloned());

    let state = all_orders(&records);
    // Concurrent joins are ordered by record ID; the first two get in.
    let mut by_id = joins.clone();
    by_id.sort_by_key(Record::id);
    let admitted: Vec<_> = by_id.iter().take(2).map(Record::author).collect();
    assert_eq!(state.members, [actor(0), admitted[0], admitted[1]].into());
    assert_eq!(state.invites[&twice.id()].uses, 2);
}

#[test]
fn joins_after_the_expiry_are_refused() {
    let g = genesis();
    let invite = invite(0, 1, vec![g.id()], 1_000, 0);
    let in_time = rec_ts(1, 0, vec![invite.id()], 1_000, Action::Join { invite: invite.id() });
    let late = rec_ts(2, 0, vec![invite.id()], 1_001, Action::Join { invite: invite.id() });

    let state = all_orders(&[g, invite, in_time, late]);
    assert_eq!(state.members, [actor(0), actor(1)].into());
}

#[test]
fn revoked_invites_stop_admitting() {
    let (mut records, heads) = founded(&[1, 2]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let link = invite(1, 1, vec![moderator.id()], 0, 0);
    let before = join(3, 0, &link);
    let revoke =
        rec(1, 2, vec![link.id(), before.id()], Action::RevokeInvite { invite: link.id() });
    let after = rec(4, 0, vec![revoke.id(), link.id()], Action::Join { invite: link.id() });
    // A plain member can't revoke someone else's invite.
    let by_member = rec(2, 1, vec![link.id()], Action::RevokeInvite { invite: link.id() });
    records.extend([moderator, link.clone(), before, revoke.clone(), after, by_member.clone()]);

    let state = all_orders(&records);
    assert!(state.accepted.contains(&revoke.id()));
    assert!(!state.accepted.contains(&by_member.id()));
    assert!(state.invites[&link.id()].revoked);
    assert!(state.is_member(actor(3)), "joined before the revoke");
    assert!(!state.is_member(actor(4)));
}

#[test]
fn a_higher_rank_revokes_anyone_elses_invite() {
    let (mut records, heads) = founded(&[1]);
    let moderator = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    let link = invite(1, 1, vec![moderator.id()], 0, 0);
    let revoke = rec(0, 3, vec![link.id()], Action::RevokeInvite { invite: link.id() });
    let again = rec(1, 2, vec![revoke.id()], Action::RevokeInvite { invite: link.id() });
    records.extend([moderator, link.clone(), revoke.clone(), again.clone()]);

    let state = all_orders(&records);
    assert!(state.accepted.contains(&revoke.id()));
    assert!(!state.accepted.contains(&again.id()), "already revoked");
}

#[test]
fn a_join_concurrent_with_a_revoke_is_settled_by_linear_order() {
    let g = genesis();
    let link = invite(0, 1, vec![g.id()], 0, 0);
    let revoke = rec(0, 2, vec![link.id()], Action::RevokeInvite { invite: link.id() });
    let racer = join(1, 0, &link);

    let state = all_orders(&[g, link, revoke.clone(), racer.clone()]);
    assert_eq!(state.is_member(actor(1)), racer.id() < revoke.id());
}

#[test]
fn a_kick_blocks_rejoining() {
    let (mut records, heads) = founded(&[1, 2]);
    let kick = rec(0, 2, heads.clone(), Action::Kick { target: actor(1) });
    let link = invite(0, 3, vec![kick.id()], 0, 0);
    let rejoin = join(1, 1, &link);
    // Kicking a key that never joined bans it in advance.
    let ban = rec(0, 4, vec![link.id()], Action::Kick { target: actor(5) });
    let banned = rec(5, 0, vec![ban.id(), link.id()], Action::Join { invite: link.id() });
    // The owner and members can't join again.
    let owner = join(0, 5, &link);
    let twice = rec(2, 1, vec![link.id()], Action::Join { invite: link.id() });
    records.extend([kick, link.clone(), rejoin.clone(), ban, banned.clone(), owner.clone()]);
    records.push(twice.clone());

    let state = all_orders(&records);
    assert!(state.kicked.contains(&actor(1)) && state.kicked.contains(&actor(5)));
    assert_eq!(state.members, [actor(0), actor(2)].into());
    for refused in [rejoin, banned, owner, twice] {
        assert!(!state.accepted.contains(&refused.id()));
    }
    assert_eq!(state.invites[&link.id()].uses, 0);
}

#[test]
fn a_kicked_moderators_invites_stop_working() {
    let (mut records, heads) = founded(&[1]);
    let moderator = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    let link = invite(1, 1, vec![moderator.id()], 0, 0);
    let early = join(2, 0, &link);
    let kick = rec(0, 3, vec![link.id(), early.id()], Action::Kick { target: actor(1) });
    let late = rec(3, 0, vec![kick.id(), link.id()], Action::Join { invite: link.id() });
    records.extend([moderator, link, early, kick, late]);

    let state = all_orders(&records);
    assert!(state.is_member(actor(2)));
    assert!(!state.is_member(actor(3)));
}

#[test]
fn a_revoke_voids_invites_signed_on_pre_revoke_heads() {
    let (mut records, heads) = founded(&[1]);
    let moderator = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    let revoke = rec(0, 3, vec![moderator.id()], Action::Revoke { target: actor(1) });
    // Signed after the revoke, pointing at the heads from before it.
    let backdated = invite(1, 1, vec![moderator.id()], 0, 0);
    let via = join(2, 0, &backdated);
    records.extend([moderator, revoke, backdated.clone(), via]);

    let state = all_orders(&records);
    assert!(!state.accepted.contains(&backdated.id()));
    assert!(!state.is_member(actor(2)));
    assert!(state.is_member(actor(1)), "demoted, still a member");
}

#[test]
fn a_kick_voids_a_concurrent_join_and_its_posts() {
    let g = genesis();
    let link = invite(0, 1, vec![g.id()], 0, 0);
    let joined = join(1, 0, &link);
    let post = rec(1, 1, vec![joined.id()], Action::Post { text: "hi".into() });
    // The owner kicks the key without having seen the join.
    let kick = rec(0, 2, vec![link.id()], Action::Kick { target: actor(1) });

    let state = all_orders(&[g, link, joined.clone(), post, kick]);
    assert!(!state.accepted.contains(&joined.id()));
    assert!(state.posts.is_empty());
    assert!(!state.is_member(actor(1)));
}

#[test]
fn hostile_membership_bodies_are_never_accepted() {
    let g = genesis();
    let link = invite(0, 1, vec![g.id()], 0, 0);
    let raw = |author: u8, kind: Kind, body: Vec<u8>| {
        Draft {
            server: server(),
            scope: Scope::Control,
            seq: 9,
            deps: vec![link.id()],
            auth_deps: vec![],
            ts: 0,
            kind,
            body,
        }
        .sign(&key(author))
        .unwrap()
    };
    let hostile = [
        raw(0, Kind::Invite, vec![0; 11]),
        raw(0, Kind::Invite, vec![0; 13]),
        raw(0, Kind::RevokeInvite, link.id().as_bytes()[..31].to_vec()),
        raw(1, Kind::Join, vec![]),
        raw(2, Kind::Join, [&link.id().as_bytes()[..], &[0]].concat()),
    ];
    for record in &hostile {
        assert_eq!(Action::from_record(record), None, "{:?} parsed", record.kind());
    }
    let mut records = vec![g, link.clone()];
    records.extend(hostile.iter().cloned());
    let state = all_orders(&records);
    assert_eq!(state.accepted.len(), 2);
    assert_eq!(state.members, [actor(0)].into());
    assert!(!state.invites[&link.id()].revoked);
}

#[test]
fn membership_bodies_round_trip() {
    let g = genesis();
    let actions = [
        Action::Invite { expires_ms: u64::MAX, max_uses: u32::MAX },
        Action::RevokeInvite { invite: g.id() },
        Action::Join { invite: g.id() },
    ];
    for action in actions {
        let record = rec(1, 0, vec![g.id()], action.clone());
        assert_eq!(record.kind(), action.kind());
        assert_eq!(Action::from_record(&record), Some(action));
    }
}
