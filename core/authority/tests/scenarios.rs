use authority::{resolve, Action, Record, Role};

fn genesis() -> Record {
    Record::new(0, 0, vec![], Action::Genesis)
}

#[test]
fn owner_grants_roles() {
    let g = genesis();
    let admin = Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Admin });
    let moderator =
        Record::new(1, 0, vec![admin.id], Action::Grant { target: 2, role: Role::Moderator });

    let state = resolve(&[g, admin, moderator]);

    assert_eq!(state.owner, Some(0));
    assert_eq!(state.role(1), Role::Admin);
    assert_eq!(state.role(2), Role::Moderator);
}

#[test]
fn kick_voids_posts_the_kicker_had_not_seen() {
    let g = genesis();
    let moderator =
        Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Moderator });
    let seen_post = Record::new(2, 0, vec![g.id], Action::Post { text: "before".into() });
    let kick = Record::new(1, 0, vec![moderator.id, seen_post.id], Action::Kick { target: 2 });
    let unseen_post = Record::new(2, 1, vec![seen_post.id], Action::Post { text: "after".into() });

    let state = resolve(&[g, moderator, seen_post.clone(), kick, unseen_post]);

    assert!(state.kicked.contains(&2));
    assert_eq!(state.posts, vec![seen_post.id]);
}

#[test]
fn revoked_admin_cannot_backdate_a_kick() {
    let g = genesis();
    let admin = Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Admin });
    let revoke = Record::new(0, 2, vec![admin.id], Action::Revoke { target: 1 });
    // Signed after the revoke, but with deps pointing at state from before it.
    let backdated = Record::new(1, 0, vec![admin.id], Action::Kick { target: 3 });

    let state = resolve(&[g, admin, revoke, backdated.clone()]);

    assert_eq!(state.role(1), Role::Member);
    assert!(!state.kicked.contains(&3));
    assert!(!state.accepted.contains(&backdated.id));
}

#[test]
fn revoked_admin_loses_permission_for_later_actions() {
    let g = genesis();
    let admin = Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Admin });
    let revoke = Record::new(0, 2, vec![admin.id], Action::Revoke { target: 1 });
    let after = Record::new(1, 0, vec![revoke.id], Action::Grant { target: 2, role: Role::Moderator });

    let state = resolve(&[g, admin, revoke, after]);

    assert_eq!(state.role(2), Role::Member);
}

#[test]
fn owner_cannot_be_kicked_or_demoted() {
    let g = genesis();
    let admin = Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Admin });
    let kick = Record::new(1, 0, vec![admin.id], Action::Kick { target: 0 });
    let revoke = Record::new(1, 1, vec![kick.id], Action::Revoke { target: 0 });

    let state = resolve(&[g, admin, kick, revoke]);

    assert_eq!(state.role(0), Role::Owner);
    assert!(state.kicked.is_empty());
}

#[test]
fn equal_ranks_cannot_remove_each_other() {
    let g = genesis();
    let a = Record::new(0, 1, vec![g.id], Action::Grant { target: 1, role: Role::Admin });
    let b = Record::new(0, 2, vec![a.id], Action::Grant { target: 2, role: Role::Admin });
    let kick = Record::new(1, 0, vec![b.id], Action::Kick { target: 2 });

    let state = resolve(&[g, a, b, kick]);

    assert_eq!(state.role(2), Role::Admin);
}

#[test]
fn records_with_missing_deps_are_ignored() {
    let g = genesis();
    let orphan = Record::new(1, 0, vec![12345], Action::Post { text: "lost".into() });

    let state = resolve(&[g, orphan]);

    assert!(state.posts.is_empty());
}
