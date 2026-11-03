//! Undoing a kick (spec B11, ש-4): the owner unbans a kicked key, which may
//! then join again through an invite, and the list of removed accounts.
//! Every test runs the reference `resolve` and the incremental `Resolver`
//! over many delivery orders and requires them to agree.

mod common;

use authority::{resolve, Action, Record, RecordId, Resolver, Role, State};
use common::{actor, founded, key, rec, server};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use records::{Draft, Kind, Scope};

const ORDERS: u64 = 200;

fn all_orders(records: &[Record]) -> State {
    let expected = resolve(server(), records);
    let mut rng = StdRng::seed_from_u64(11);
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

fn invite(author: u8, seq: u64, deps: Vec<RecordId>) -> Record {
    rec(author, seq, deps, Action::Invite { expires_ms: 0, max_uses: 0 })
}

fn join(author: u8, seq: u64, deps: Vec<RecordId>, invite: &Record) -> Record {
    let mut deps = deps;
    deps.push(invite.id());
    rec(author, seq, deps, Action::Join { invite: invite.id() })
}

#[test]
fn the_owner_unbans_and_the_key_rejoins_through_an_invite() {
    let (mut records, heads) = founded(&[1]);
    let kick = rec(0, 2, heads.clone(), Action::kick(actor(1)));
    let unban = rec(0, 3, vec![kick.id()], Action::Unban { target: actor(1) });
    records.extend([kick.clone(), unban.clone()]);

    let state = all_orders(&records);
    assert!(state.accepted.contains(&unban.id()));
    assert!(!state.kicked.contains(&actor(1)) && state.removed.is_empty());
    assert!(!state.is_member(actor(1)), "an unban doesn't make them a member");

    let link = invite(0, 4, vec![unban.id()]);
    let rejoin = join(1, 1, vec![unban.id()], &link);
    let post = rec(1, 2, vec![rejoin.id()], Action::Post { text: "back".into() });
    records.extend([link, rejoin.clone(), post.clone()]);
    let state = all_orders(&records);
    assert!(state.is_member(actor(1)));
    assert!(state.accepted.contains(&rejoin.id()) && state.accepted.contains(&post.id()));
}

#[test]
fn only_the_owner_unbans_and_only_a_kicked_key() {
    let (mut records, heads) = founded(&[1, 2]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(2), role: Role::Admin });
    let kick = rec(0, 3, vec![admin.id()], Action::kick(actor(1)));
    let by_admin = rec(2, 1, vec![kick.id()], Action::Unban { target: actor(1) });
    let by_kicked = rec(1, 1, vec![kick.id()], Action::Unban { target: actor(1) });
    let not_kicked = rec(0, 4, vec![kick.id()], Action::Unban { target: actor(2) });
    records.extend([admin, kick, by_admin.clone(), by_kicked.clone(), not_kicked.clone()]);

    let state = all_orders(&records);
    assert!(state.kicked.contains(&actor(1)));
    for refused in [by_admin, by_kicked, not_kicked] {
        assert!(!state.accepted.contains(&refused.id()));
    }
    assert_eq!(state.role(actor(2)), Role::Admin, "a refused unban changes nothing");
}

#[test]
fn removed_accounts_list_each_kick_until_undone() {
    let (mut records, heads) = founded(&[1, 2]);
    let kick = rec(0, 2, heads.clone(), Action::kick(actor(1)));
    // Kicking a key that never joined bans it in advance; it is listed too.
    let ban = rec(0, 3, vec![kick.id()], Action::kick(actor(7)));
    records.extend([kick.clone(), ban.clone()]);
    let state = all_orders(&records);
    assert_eq!(state.removed, [(actor(1), kick.id()), (actor(7), ban.id())].into());

    let unban = rec(0, 4, vec![ban.id()], Action::Unban { target: actor(1) });
    records.push(unban);
    let state = all_orders(&records);
    assert_eq!(state.removed, [(actor(7), ban.id())].into());

    // Kicked again later: listed with the new kick.
    let again = rec(0, 5, vec![records.last().unwrap().id()], Action::kick(actor(1)));
    records.push(again.clone());
    let state = all_orders(&records);
    assert_eq!(state.removed, [(actor(1), again.id()), (actor(7), ban.id())].into());
}

/// An unban ends the kick only for records signed after it. Records the
/// kicked key signed on pre-kick heads stay voided.
#[test]
fn an_unban_does_not_revive_backdated_records() {
    let (mut records, heads) = founded(&[1]);
    let kick = rec(0, 2, heads.clone(), Action::kick(actor(1)));
    let unban = rec(0, 3, vec![kick.id()], Action::Unban { target: actor(1) });
    let backdated = rec(1, 1, heads.clone(), Action::Post { text: "old heads".into() });
    records.extend([kick, unban, backdated.clone()]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&backdated.id()));
}

/// A record that descends from a revoke was signed knowing it: after a new
/// grant the moderator acts again (no longer voided by the old revoke).
#[test]
fn a_regranted_moderator_acts_again() {
    let (mut records, heads) = founded(&[1, 2]);
    let moderator = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    let revoke = rec(0, 3, vec![moderator.id()], Action::revoke(actor(1)));
    let regrant =
        rec(0, 4, vec![revoke.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    let kick = rec(1, 1, vec![regrant.id()], Action::kick(actor(2)));
    // Signed on pre-revoke heads: still voided.
    let backdated = rec(1, 2, vec![moderator.id()], Action::kick(actor(2)));
    records.extend([moderator, revoke, regrant, kick.clone(), backdated.clone()]);
    let state = all_orders(&records);
    assert!(state.accepted.contains(&kick.id()));
    assert!(!state.accepted.contains(&backdated.id()));
    assert!(state.kicked.contains(&actor(2)));
}

#[test]
fn hostile_unban_bodies_are_never_accepted() {
    let (mut records, heads) = founded(&[1]);
    let kick = rec(0, 2, heads, Action::kick(actor(1)));
    let target = actor(1).as_bytes().to_vec();
    let bodies = [vec![], target[..31].to_vec(), [target.clone(), vec![0]].concat()];
    let mut refused = Vec::new();
    for (seq, body) in bodies.into_iter().enumerate() {
        let record = Draft {
            server: server(),
            scope: Scope::Control,
            seq: 10 + seq as u64,
            deps: vec![kick.id()],
            auth_deps: vec![],
            ts: 0,
            kind: Kind::Unban,
            body,
        }
        .sign(&key(0))
        .unwrap();
        assert_eq!(Action::from_record(&record), None);
        refused.push(record);
    }
    let good = rec(0, 20, vec![kick.id()], Action::Unban { target: actor(1) });
    assert_eq!(Action::from_record(&good), Some(Action::Unban { target: actor(1) }));
    records.push(kick);
    records.extend(refused.iter().cloned());
    let state = all_orders(&records);
    assert!(state.kicked.contains(&actor(1)));
}

/// D28: a demotion is a revoke, then a new grant of the lower role. The
/// demoted admin keeps the moderator's powers it was granted after the
/// revoke, and loses the admin's.
#[test]
fn demoting_an_admin_to_moderator_is_a_revoke_then_a_grant() {
    let (mut records, heads) = founded(&[1, 2, 3]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let moderator =
        rec(0, 3, vec![admin.id()], Action::Grant { target: actor(2), role: Role::Moderator });
    let revoke = rec(0, 4, vec![moderator.id()], Action::revoke(actor(1)));
    let demoted =
        rec(0, 5, vec![revoke.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    // As a moderator now: kicking a member works, kicking a moderator doesn't.
    let kick_member = rec(1, 1, vec![demoted.id()], Action::kick(actor(3)));
    let kick_moderator = rec(1, 2, vec![kick_member.id()], Action::kick(actor(2)));
    records.extend([
        admin,
        moderator,
        revoke,
        demoted,
        kick_member.clone(),
        kick_moderator.clone(),
    ]);

    let state = all_orders(&records);
    assert_eq!(state.role(actor(1)), Role::Moderator);
    assert!(state.accepted.contains(&kick_member.id()));
    assert!(!state.accepted.contains(&kick_moderator.id()));
    assert!(state.kicked.contains(&actor(3)) && !state.kicked.contains(&actor(2)));
}

/// Records that descend from a removal are judged by the state there, which
/// still refuses them while it lasts.
#[test]
fn records_signed_after_a_removal_are_refused_by_state() {
    let (mut records, heads) = founded(&[1, 2]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let revoke = rec(0, 3, vec![moderator.id()], Action::revoke(actor(1)));
    // A demoted moderator's kick that knows the demotion: refused.
    let demoted_kick = rec(1, 1, vec![revoke.id()], Action::kick(actor(2)));
    let kick = rec(0, 4, vec![demoted_kick.id()], Action::kick(actor(2)));
    // The kicked actor's post that knows the kick: refused.
    let kicked_post = rec(2, 1, vec![kick.id()], Action::Post { text: "still here".into() });
    records.extend([moderator, revoke, demoted_kick.clone(), kick.clone(), kicked_post.clone()]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&demoted_kick.id()));
    assert!(!state.accepted.contains(&kicked_post.id()));
    assert!(state.accepted.contains(&kick.id()));
}

/// A re-granted moderator's new invite admits people; the invites they
/// signed before the revoke stay dead.
#[test]
fn a_regranted_moderators_new_invite_works() {
    let (mut records, heads) = founded(&[1]);
    let moderator = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Moderator });
    let old_link = invite(1, 1, vec![moderator.id()]);
    let revoke = rec(0, 3, vec![old_link.id()], Action::revoke(actor(1)));
    let regrant =
        rec(0, 4, vec![revoke.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    let new_link = invite(1, 2, vec![regrant.id()]);
    let through_new = join(3, 0, vec![], &new_link);
    let through_old = join(4, 0, vec![regrant.id()], &old_link);
    records.extend([
        moderator,
        old_link,
        revoke,
        regrant,
        new_link,
        through_new.clone(),
        through_old.clone(),
    ]);
    let state = all_orders(&records);
    assert!(state.is_member(actor(3)), "the new invite works");
    assert!(!state.is_member(actor(4)), "the pre-revoke invite stays dead");
}
