//! Attacks by hostile peers. Each test checks that both the reference
//! [`resolve`] and the incremental [`Resolver`] hold up.

mod common;

use std::time::{Duration, Instant};

use authority::{
    resolve, sign_control, sign_genesis, Action, Inserted, Record, RecordId, Resolver, Role, State,
};
use common::{actor, genesis, key, rec, server};
use records::{Draft, Kind, RecordError, Scope, GENESIS_NONCE_LEN};

/// Both resolvers on the test server, records delivered in the given order.
fn both(records: &[Record]) -> State {
    let state = resolve(server(), records);
    let mut resolver = Resolver::new(server());
    for record in records {
        resolver.insert(record.clone());
    }
    assert_eq!(resolver.state(), &state, "incremental disagrees with resolve");
    state
}

#[test]
fn a_forged_genesis_cannot_take_the_server() {
    let attacker = 9;
    let g = genesis();
    let post = rec(0, 1, vec![g.id()], Action::Post { text: "real".into() });

    // Signing a genesis that claims the victim's server ID fails...
    let forged = Draft {
        server: server(),
        scope: Scope::Control,
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 0,
        kind: Kind::Genesis,
        body: vec![0; GENESIS_NONCE_LEN],
    };
    assert_eq!(forged.sign(&key(attacker)), Err(RecordError::GenesisServerMismatch));

    // ...and grinding nonces only produces geneses of other servers. Take
    // the one with the smallest ID, the one that used to win ownership.
    let rival = (0..2000u16)
        .map(|n| {
            let mut nonce = [0; GENESIS_NONCE_LEN];
            nonce[..2].copy_from_slice(&n.to_le_bytes());
            sign_genesis(&key(attacker), nonce, 0, b"mine now").unwrap()
        })
        .min_by_key(Record::id)
        .unwrap();
    assert!(rival.id() < g.id(), "grinding should beat the real genesis ID");
    assert_ne!(rival.server(), server());
    // The attacker's own records hanging off the rival genesis.
    let grab = sign_control(
        &key(attacker),
        rival.server(),
        1,
        vec![rival.id()],
        0,
        &Action::Kick { target: actor(0) },
    )
    .unwrap();

    for records in [
        vec![rival.clone(), grab.clone(), g.clone(), post.clone()],
        vec![g.clone(), post.clone(), rival.clone(), grab.clone()],
    ] {
        let state = both(&records);
        assert_eq!(state.owner, Some(actor(0)));
        assert_eq!(state.accepted.len(), 2);
        assert!(state.kicked.is_empty());
    }
}

#[test]
fn another_servers_records_without_deps_are_ignored() {
    let g = genesis();
    // A real server B whose genesis sorts ahead of ours, and a dep-less post.
    let b_records: Vec<Record> = (0..200u8)
        .map(|n| sign_genesis(&key(3), [n; GENESIS_NONCE_LEN], 0, b"server B").unwrap())
        .filter(|b| b.id() < g.id())
        .take(1)
        .flat_map(|b| {
            let post = sign_control(
                &key(3),
                b.server(),
                1,
                vec![],
                0,
                &Action::Post { text: "dep-less".into() },
            )
            .unwrap();
            [b, post]
        })
        .collect();
    assert_eq!(b_records.len(), 2);

    let mut resolver = Resolver::new(server());
    for record in &b_records {
        assert_eq!(resolver.insert(record.clone()), Inserted::Ignored);
    }
    assert_eq!(resolver.insert(g.clone()), Inserted::Joined);

    let mut records = b_records.clone();
    records.push(g.clone());
    let state = resolve(server(), &records);
    assert_eq!(state.owner, Some(actor(0)));
    assert_eq!(state.accepted, [g.id()].into());
    assert_eq!(resolver.state(), &state);
}

#[test]
fn demoting_by_grant_is_refused_so_backdating_stays_blocked() {
    let g = genesis();
    let moderator =
        rec(0, 1, vec![g.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    let admin =
        rec(0, 2, vec![moderator.id()], Action::Grant { target: actor(2), role: Role::Admin });
    // Demoting with a grant would skip the voiding a revoke does.
    let demote =
        rec(0, 3, vec![admin.id()], Action::Grant { target: actor(1), role: Role::Member });
    let lower =
        rec(0, 4, vec![demote.id()], Action::Grant { target: actor(2), role: Role::Moderator });
    let state = both(&[g.clone(), moderator.clone(), admin.clone(), demote.clone(), lower.clone()]);
    assert!(!state.accepted.contains(&demote.id()));
    assert!(!state.accepted.contains(&lower.id()));
    assert_eq!(state.role(actor(1)), Role::Moderator);
    assert_eq!(state.role(actor(2)), Role::Admin);

    // The way to demote is a revoke, which voids a backdated kick.
    let revoke = rec(0, 3, vec![admin.id()], Action::Revoke { target: actor(1) });
    let backdated = rec(1, 0, vec![moderator.id()], Action::Kick { target: actor(5) });
    let state = both(&[g, moderator, admin, revoke, backdated.clone()]);
    assert_eq!(state.role(actor(1)), Role::Member);
    assert!(!state.accepted.contains(&backdated.id()));
    assert!(!state.kicked.contains(&actor(5)));
}

#[test]
fn a_kicked_actor_cannot_be_kicked_again() {
    let g = genesis();
    let moderator =
        rec(0, 1, vec![g.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    let kick = rec(1, 0, vec![moderator.id()], Action::Kick { target: actor(2) });
    let again = rec(1, 1, vec![kick.id()], Action::Kick { target: actor(2) });
    let state = both(&[g, moderator, kick.clone(), again.clone()]);
    assert!(state.accepted.contains(&kick.id()));
    assert!(!state.accepted.contains(&again.id()));
}

#[test]
fn kicked_actors_cannot_act() {
    let g = genesis();
    let admin = rec(0, 1, vec![g.id()], Action::Grant { target: actor(1), role: Role::Admin });
    let kick = rec(0, 2, vec![admin.id()], Action::Kick { target: actor(1) });
    // After seeing the kick, and concurrently with it.
    let after = rec(1, 0, vec![kick.id()], Action::Post { text: "still here".into() });
    let grant =
        rec(1, 1, vec![after.id()], Action::Grant { target: actor(3), role: Role::Moderator });
    let concurrent = rec(1, 2, vec![admin.id()], Action::Kick { target: actor(4) });
    let regrant = rec(0, 3, vec![kick.id()], Action::Grant { target: actor(1), role: Role::Admin });
    let state = both(&[g, admin, kick, after, grant, concurrent, regrant.clone()]);
    assert_eq!(state.accepted.len(), 3, "only genesis, grant and kick");
    assert!(state.kicked.contains(&actor(1)));
    assert!(!state.kicked.contains(&actor(4)));
    assert_eq!(state.role(actor(3)), Role::Member);
    assert!(!state.accepted.contains(&regrant.id()));
}

#[test]
fn records_waiting_on_missing_deps_are_bounded() {
    let g = genesis();
    let mut resolver = Resolver::with_pending_limit(server(), 3);
    let orphan = |n: u8| {
        rec(
            5,
            u64::from(n),
            vec![RecordId::from_bytes([n; 32])],
            Action::Post { text: "never".into() },
        )
    };
    for n in 0..3 {
        assert_eq!(resolver.insert(orphan(n)), Inserted::Pending);
    }
    assert_eq!(resolver.insert(orphan(3)), Inserted::Dropped);
    assert_eq!(resolver.pending(), 3);

    // While full, a record waiting on our genesis is dropped too; once the
    // genesis joins, delivering it again works.
    let post = rec(1, 0, vec![g.id()], Action::Post { text: "hi".into() });
    assert_eq!(resolver.insert(post.clone()), Inserted::Dropped);
    assert_eq!(resolver.insert(g.clone()), Inserted::Joined);
    assert_eq!(resolver.insert(post.clone()), Inserted::Joined);
    assert_eq!(resolver.insert(post.clone()), Inserted::Ignored);
    assert_eq!(resolver.state(), &resolve(server(), &[g, post]));
}

/// Hostile logs that used to cost quadratic time or memory.
#[test]
fn large_hostile_logs_resolve_quickly() {
    let g = genesis();

    // A long chain hanging off a dep that never arrives: every record in it
    // used to cost a full scan of the set.
    let mut chain = vec![g.clone()];
    let mut prev = RecordId::from_bytes([1; 32]);
    for seq in 0..20_000 {
        let post = rec(2, seq, vec![prev], Action::Post { text: String::new() });
        prev = post.id();
        chain.push(post);
    }
    let start = Instant::now();
    let state = resolve(server(), &chain);
    let broken = start.elapsed();
    assert_eq!(state.accepted, [g.id()].into());

    // Many posts by ordinary members: every record used to get an ancestor
    // bit set as long as the log (50,000^2 bits = 312 MB).
    let moderator =
        rec(0, 1, vec![g.id()], Action::Grant { target: actor(1), role: Role::Moderator });
    let mut log = vec![g.clone(), moderator.clone()];
    let mut heads = vec![moderator.id()];
    for seq in 0..50_000u64 {
        let author = 2 + (seq % 4) as u8;
        let post = rec(author, seq, heads.clone(), Action::Post { text: String::new() });
        if seq % 3 == 0 {
            heads = vec![post.id()];
        } else {
            heads.push(post.id());
            heads.truncate(4);
        }
        log.push(post);
    }
    let kick = rec(1, 0, vec![moderator.id()], Action::Kick { target: actor(2) });
    log.push(kick.clone());
    let start = Instant::now();
    let state = resolve(server(), &log);
    let wide = start.elapsed();
    assert!(state.kicked.contains(&actor(2)));
    // actor 2's posts were all concurrent with the kick and are voided.
    assert_eq!(state.posts.len(), 50_000 - 12_500);

    println!("broken chain of 20000: {broken:?}; 50000 posts + kick: {wide:?}");
    assert!(broken < Duration::from_secs(2), "broken chain took {broken:?}");
    assert!(wide < Duration::from_secs(10), "wide log took {wide:?}");
}
