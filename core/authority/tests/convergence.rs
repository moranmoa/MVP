//! Spike experiment 3: peers that receive the same records in any order must
//! end up with the same server state, whether they recompute from scratch
//! ([`resolve`]) or apply records incrementally ([`Resolver`]).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use authority::{
    resolve, Action, ChannelId, Cut, Inserted, Record, RecordId, Resolver, Role, State, Stats,
};
use common::{actor, genesis, rec_ts, server};
mod rng;
use records::Kind;
use rng::{SliceRandom, StdRng};

const ACTORS: u8 = 5;
const SCENARIOS: u64 = 10_000;
const DELIVERIES_PER_SCENARIO: usize = 3;

/// A random action; joins and revoke-invites name one of `invites`, the
/// invites the author has seen.
fn random_action(rng: &mut StdRng, author: u8, invites: &[RecordId], cuts: bool) -> Action {
    let cut = |rng: &mut StdRng| if cuts { random_cut(rng) } else { Cut::default() };
    let target = actor(rng.gen_range(0..ACTORS));
    match rng.gen_range(0..8) {
        0 => {
            let role = *[Role::Member, Role::Moderator, Role::Admin].choose(rng).unwrap();
            Action::Grant { target, role }
        }
        1 => Action::Revoke { target, cut: cut(rng) },
        2 => Action::Kick { target, cut: cut(rng) },
        3 if cuts && rng.gen_bool(0.5) => Action::AgeClaim { adult: rng.gen_bool(0.5) },
        3 if cuts => Action::ChannelCreate {
            channel: ChannelId::from_bytes([rng.gen_range(0..3); 32]),
            adult: rng.gen_bool(0.2),
            name: format!("c{author}"),
        },
        5 => Action::Invite {
            expires_ms: *[0, 30, 60].choose(rng).unwrap(),
            max_uses: rng.gen_range(0..3),
        },
        6 | 7 if !invites.is_empty() => {
            let invite = *invites.choose(rng).unwrap();
            if rng.gen_bool(0.75) {
                Action::Join { invite }
            } else {
                Action::RevokeInvite { invite }
            }
        }
        _ => Action::Post { text: format!("from {author}") },
    }
}

/// Up to 3 channel heads, two of them sometimes in one channel, so cuts go through the wire format.
fn random_cut(rng: &mut StdRng) -> Cut {
    let heads = (0..rng.gen_range(0..4u8))
        .map(|n| (ChannelId::from_bytes([n / 2; 32]), RecordId::from_bytes([rng.gen(); 32])))
        .collect();
    Cut::new(heads).expect("at most 3 heads")
}

/// Records in `known` that no other known record depends on.
fn heads(known: &BTreeSet<RecordId>, all: &BTreeMap<RecordId, Record>) -> Vec<RecordId> {
    let referenced: BTreeSet<RecordId> =
        known.iter().flat_map(|id| all[id].deps().iter().copied()).collect();
    known.iter().copied().filter(|id| !referenced.contains(id)).collect()
}

/// Actors write records against their own partial view of the log and
/// occasionally sync with each other, so plenty of records are concurrent.
/// Kicks and revokes carry random cuts if `cuts`, and channels are created
/// (the owner's may win, the rest are refused) and ages claimed too. Returns the records in
/// the order they were written.
fn random_scenario(rng: &mut StdRng, steps: usize, cuts: bool) -> Vec<Record> {
    let genesis = genesis();
    let mut written = vec![genesis.clone()];
    let mut all = BTreeMap::from([(genesis.id(), genesis.clone())]);
    let mut known: Vec<BTreeSet<RecordId>> =
        (0..ACTORS).map(|_| BTreeSet::from([genesis.id()])).collect();
    let mut seq = vec![1u64; usize::from(ACTORS)];

    for _ in 0..steps {
        let actor = rng.gen_range(0..ACTORS);
        let a = usize::from(actor);
        if rng.gen_bool(0.3) {
            let other = rng.gen_range(0..usize::from(ACTORS));
            let theirs = known[other].clone();
            known[a].extend(theirs);
        } else {
            let mut deps = heads(&known[a], &all);
            let invites: Vec<RecordId> =
                known[a].iter().copied().filter(|id| all[id].kind() == Kind::Invite).collect();
            let action = random_action(rng, actor, &invites, cuts);
            if let Action::Join { invite } | Action::RevokeInvite { invite } = &action {
                deps.push(*invite);
            }
            // Timestamps around the invite expiries above.
            let record = rec_ts(actor, seq[a], deps, rng.gen_range(0..100), action);
            seq[a] += 1;
            known[a].insert(record.id());
            all.insert(record.id(), record.clone());
            written.push(record);
        }
    }
    written
}

/// A peer receives records one by one in random order. After each arrival
/// the incremental resolver must agree with recomputing from scratch, the way
/// the spike's client did.
fn deliver_in_random_order(records: &[Record], rng: &mut StdRng, seed: u64) -> (State, Stats) {
    let mut arriving = records.to_vec();
    arriving.shuffle(rng);
    let mut held = Vec::with_capacity(arriving.len());
    let mut resolver = Resolver::new(server());
    for record in arriving {
        held.push(record.clone());
        resolver.insert(record);
        assert_eq!(
            resolver.state(),
            &resolve(server(), &held),
            "incremental diverged on seed {seed}"
        );
    }
    (resolver.state().clone(), resolver.stats())
}

#[test]
fn all_delivery_orders_converge() {
    let mut accepted_removals = 0usize;
    let mut joined_members = 0usize;
    let mut used_up_or_revoked = 0usize;
    let mut totals = Stats::default();
    for seed in 0..SCENARIOS {
        let mut rng = StdRng::seed_from_u64(seed);
        let steps = rng.gen_range(5..40);
        let records = random_scenario(&mut rng, steps, true);
        let expected = resolve(server(), &records);
        accepted_removals += expected.kicked.len();
        joined_members += expected.members.len().saturating_sub(1);
        used_up_or_revoked += expected
            .invites
            .values()
            .filter(|i| i.revoked || (i.max_uses != 0 && i.uses == i.max_uses))
            .count();

        for _ in 0..DELIVERIES_PER_SCENARIO {
            let (state, stats) = deliver_in_random_order(&records, &mut rng, seed);
            assert_eq!(state, expected, "diverged on seed {seed}");
            totals.joined += stats.joined;
            totals.incremental += stats.incremental;
            totals.rebuilds += stats.rebuilds;
            totals.rebuilds_for_voids += stats.rebuilds_for_voids;
        }
    }
    // Guard against a generator that never exercises the hard cases.
    assert!(accepted_removals > 0, "no scenario accepted a kick");
    assert!(joined_members > 0, "no scenario accepted a join");
    assert!(used_up_or_revoked > 0, "no invite was used up or revoked");
    println!(
        "{SCENARIOS} scenarios x {DELIVERIES_PER_SCENARIO} delivery orders converged, \
         incremental == resolve(held) after every arrival; {accepted_removals} kicks, \
         {joined_members} members joined, {used_up_or_revoked} invites used up or revoked \
         overall; {} records joined the log, {} applied incrementally, {} full re-resolves ({} of them for voids)",
        totals.joined, totals.incremental, totals.rebuilds, totals.rebuilds_for_voids
    );
}

/// Delivers `records` in order. Records evicted from the pending queue are
/// delivered again afterwards, the way a peer re-requests what it couldn't
/// keep. Returns how many redeliveries that took.
fn apply_all(records: &[Record]) -> (Resolver, Duration, usize) {
    let mut resolver = Resolver::new(server());
    let start = Instant::now();
    let mut redelivered = 0;
    let mut round: Vec<Record> = records.to_vec();
    while !round.is_empty() {
        let mut evicted = Vec::new();
        for record in round {
            if let Inserted::Pending { evicted: Some(record) } = resolver.insert(record) {
                evicted.push(*record);
            }
        }
        redelivered += evicted.len();
        round = evicted;
    }
    (resolver, start.elapsed(), redelivered)
}

#[test]
fn incremental_vs_full_on_a_large_control_log() {
    let mut rng = StdRng::seed_from_u64(42);
    // No cuts: they don't change resolution, but they change record IDs,
    // so this log stays the one the main branch is measured on.
    let written = random_scenario(&mut rng, 7_150, false);
    let n = written.len();

    let start = Instant::now();
    let expected = resolve(server(), &written);
    let full = start.elapsed();
    println!(
        "full resolve of {n} control records ({} accepted): {full:?}",
        expected.accepted.len()
    );

    // A live peer: records arrive in the order they were written.
    let (mut resolver, _, _) = apply_all(&written[..n - 1]);
    let start = Instant::now();
    resolver.insert(written[n - 1].clone());
    let last = start.elapsed();
    assert_eq!(resolver.state(), &expected);
    println!("incremental: applying record #{n} to the other {} took {last:?}", n - 1);

    let (resolver, total, _) = apply_all(&written);
    assert_eq!(resolver.state(), &expected);
    // A live peer re-resolves only for the rare removal that voids
    // applied records. Measured: 16 on this log (tests/rng, seed 42, record
    // format v2); the bound is that + 2. Record IDs fix the linear order, so
    // re-measure if the generator, the scenario or the record format change.
    assert!(
        resolver.stats().rebuilds <= 18,
        "too many re-resolves: {}",
        describe(resolver.stats())
    );
    println!(
        "incremental, written order: all {n} records in {total:?} ({:?} per record; {})",
        total / n as u32,
        describe(resolver.stats())
    );

    // Catching up after being offline: everything arrives shuffled.
    let mut shuffled = written.clone();
    shuffled.shuffle(&mut rng);
    let (resolver, total, redelivered) = apply_all(&shuffled);
    assert_eq!(resolver.state(), &expected);
    println!(
        "incremental, random order: all {n} records in {total:?} ({}; {redelivered} redelivered after eviction from the full pending queue)",
        describe(resolver.stats())
    );
}

fn describe(stats: Stats) -> String {
    format!(
        "{} joined, {} incremental, {} full re-resolves of which {} for voids",
        stats.joined, stats.incremental, stats.rebuilds, stats.rebuilds_for_voids
    )
}
