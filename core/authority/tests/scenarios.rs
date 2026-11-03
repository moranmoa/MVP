mod common;

use authority::{resolve, sign_control, Action, Record, RecordId, Resolver, Role};
use common::{actor, founded, genesis, key, rec, server};
use records::{ChannelId, Draft, Kind, Scope, ServerId};

fn with(mut records: Vec<Record>, more: impl IntoIterator<Item = Record>) -> Vec<Record> {
    records.extend(more);
    records
}

#[test]
fn owner_grants_roles() {
    let (base, heads) = founded(&[1, 2]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let moderator =
        rec(1, 1, vec![admin.id()], Action::Grant { target: actor(2), role: Role::Moderator });

    let state = resolve(server(), &with(base, [admin, moderator]));

    assert_eq!(state.owner, Some(actor(0)));
    assert_eq!(state.role(actor(1)), Role::Admin);
    assert_eq!(state.role(actor(2)), Role::Moderator);
}

#[test]
fn kick_voids_posts_the_kicker_had_not_seen() {
    let (base, heads) = founded(&[1, 2]);
    let moderator =
        rec(0, 2, heads.clone(), Action::Grant { target: actor(1), role: Role::Moderator });
    let seen_post = rec(2, 1, heads, Action::Post { text: "before".into() });
    let kick = rec(1, 1, vec![moderator.id(), seen_post.id()], Action::kick(actor(2)));
    let unseen_post = rec(2, 2, vec![seen_post.id()], Action::Post { text: "after".into() });

    let state = resolve(server(), &with(base, [moderator, seen_post.clone(), kick, unseen_post]));

    assert!(state.kicked.contains(&actor(2)));
    assert!(!state.is_member(actor(2)));
    assert_eq!(state.posts, vec![seen_post.id()]);
}

#[test]
fn revoked_admin_cannot_backdate_a_kick() {
    let (base, heads) = founded(&[1, 3]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let revoke = rec(0, 3, vec![admin.id()], Action::revoke(actor(1)));
    // Signed after the revoke, but with deps pointing at state from before it.
    let backdated = rec(1, 1, vec![admin.id()], Action::kick(actor(3)));

    let state = resolve(server(), &with(base, [admin, revoke, backdated.clone()]));

    assert_eq!(state.role(actor(1)), Role::Member);
    assert!(state.is_member(actor(1)), "a revoke keeps membership");
    assert!(!state.kicked.contains(&actor(3)));
    assert!(!state.accepted.contains(&backdated.id()));
}

#[test]
fn revoked_admin_loses_permission_for_later_actions() {
    let (base, heads) = founded(&[1, 2]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let revoke = rec(0, 3, vec![admin.id()], Action::revoke(actor(1)));
    let after =
        rec(1, 1, vec![revoke.id()], Action::Grant { target: actor(2), role: Role::Moderator });

    let state = resolve(server(), &with(base, [admin, revoke, after]));

    assert_eq!(state.role(actor(2)), Role::Member);
}

#[test]
fn owner_cannot_be_kicked_or_demoted() {
    let (base, heads) = founded(&[1]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let kick = rec(1, 1, vec![admin.id()], Action::kick(actor(0)));
    let revoke = rec(1, 2, vec![kick.id()], Action::revoke(actor(0)));

    let state = resolve(server(), &with(base, [admin, kick, revoke]));

    assert_eq!(state.role(actor(0)), Role::Owner);
    assert!(state.is_member(actor(0)));
    assert!(state.kicked.is_empty());
}

#[test]
fn equal_ranks_cannot_remove_each_other() {
    let (base, heads) = founded(&[1, 2]);
    let a = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let b = rec(0, 3, vec![a.id()], Action::Grant { target: actor(2), role: Role::Admin });
    let kick = rec(1, 1, vec![b.id()], Action::kick(actor(2)));

    let state = resolve(server(), &with(base, [a, b, kick]));

    assert_eq!(state.role(actor(2)), Role::Admin);
}

#[test]
fn records_with_missing_deps_are_ignored() {
    let (base, _) = founded(&[1]);
    let orphan =
        rec(1, 1, vec![RecordId::from_bytes([7; 32])], Action::Post { text: "lost".into() });

    let state = resolve(server(), &with(base, [orphan]));

    assert!(state.posts.is_empty());
}

#[test]
fn records_from_another_server_are_ignored() {
    let g = genesis();
    let other_server = ServerId::from_bytes([0x77; 32]);
    let foreign = sign_control(
        &key(0),
        other_server,
        1,
        vec![g.id()],
        0,
        &Action::Grant { target: actor(1), role: Role::Admin },
    )
    .unwrap();
    let child = rec(1, 0, vec![foreign.id()], Action::Post { text: "via foreign".into() });

    let records = [g, foreign, child];
    let state = resolve(server(), &records);
    assert_eq!(state.role(actor(1)), Role::Member);
    assert_eq!(state.accepted.len(), 1);

    let mut resolver = Resolver::new(server());
    for record in records {
        resolver.insert(record);
    }
    assert_eq!(resolver.state(), &state);
}

#[test]
fn unparseable_bodies_are_never_accepted() {
    let g = genesis();
    let garbage = Draft {
        server: server(),
        scope: Scope::Control,
        seq: 1,
        deps: vec![g.id()],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::Grant,
        body: vec![1, 2, 3],
    }
    .sign(&key(0))
    .unwrap();

    let state = resolve(server(), &[g, garbage.clone()]);

    assert!(!state.accepted.contains(&garbage.id()));
}

#[test]
fn resolver_ignores_duplicates_and_channel_records() {
    let (base, heads) = founded(&[1]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let message = Draft {
        server: server(),
        scope: Scope::Channel(ChannelId::from_bytes([1; 32])),
        seq: 0,
        deps: vec![],
        auth_deps: vec![admin.id()],
        ts: 0,
        kind: Kind::Message,
        body: b"hi".to_vec(),
    }
    .sign(&key(1))
    .unwrap();

    let mut resolver = Resolver::new(server());
    let mut arriving = base.clone();
    arriving.extend([admin.clone(), message, base[0].clone(), admin.clone()]);
    for record in arriving {
        resolver.insert(record);
    }

    assert_eq!(resolver.state(), &resolve(server(), &with(base.clone(), [admin])));
    assert_eq!(resolver.stats().joined, base.len() + 1);
}

#[test]
fn resolver_applies_a_live_log_without_rebuilding() {
    let (base, heads) = founded(&[1, 2, 3]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let post = rec(2, 1, vec![admin.id()], Action::Post { text: "hi".into() });
    let moderator =
        rec(1, 1, vec![post.id()], Action::Grant { target: actor(3), role: Role::Moderator });
    let kick = rec(3, 1, vec![moderator.id()], Action::kick(actor(2)));
    let records = with(base, [admin, post, moderator, kick]);

    let mut resolver = Resolver::new(server());
    for record in records.clone() {
        resolver.insert(record);
    }

    assert_eq!(resolver.state(), &resolve(server(), &records));
    assert!(resolver.state().kicked.contains(&actor(2)));
    // Only the genesis needs a (trivial) re-resolve: it creates the owner.
    assert_eq!(resolver.stats().rebuilds, 1);
}
