//! Spike experiment 3: peers that receive the same records in any order must
//! end up with the same server state, whether they recompute from scratch
//! ([`resolve`]) or apply records incrementally ([`Resolver`]).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use authority::{resolve, Action, Record, RecordId, Resolver, Role, State, Stats};
use common::{actor, rec};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

const ACTORS: u8 = 5;
const SCENARIOS: u64 = 10_000;
const DELIVERIES_PER_SCENARIO: usize = 3;

fn random_action(rng: &mut StdRng, author: u8) -> Action {
    let target = actor(rng.gen_range(0..ACTORS));
    match rng.gen_range(0..4) {
        0 => {
            let role = *[Role::Member, Role::Moderator, Role::Admin].choose(rng).unwrap();
            Action::Grant { target, role }
        }
        1 => Action::Revoke { target },
        2 => Action::Kick { target },
        _ => Action::Post { text: format!("from {author}") },
    }
}

/// Records in `known` that no other known record depends on.
fn heads(known: &BTreeSet<RecordId>, all: &BTreeMap<RecordId, Record>) -> Vec<RecordId> {
    let referenced: BTreeSet<RecordId> =
        known.iter().flat_map(|id| all[id].deps().iter().copied()).collect();
    known.iter().copied().filter(|id| !referenced.contains(id)).collect()
}

/// Actors write records against their own partial view of the log and
/// occasionally sync with each other, so plenty of records are concurrent.
/// Returns the records in the order they were written.
fn random_scenario(rng: &mut StdRng, steps: usize) -> Vec<Record> {
    let genesis = rec(0, 0, vec![], Action::Genesis);
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
            let deps = heads(&known[a], &all);
            let action = random_action(rng, actor);
            let record = rec(actor, seq[a], deps, action);
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
    let mut resolver = Resolver::new();
    for record in arriving {
        held.push(record.clone());
        resolver.insert(record);
        assert_eq!(resolver.state(), &resolve(&held), "incremental diverged on seed {seed}");
    }
    (resolver.state().clone(), resolver.stats())
}

#[test]
fn all_delivery_orders_converge() {
    let mut accepted_removals = 0usize;
    let mut totals = Stats::default();
    for seed in 0..SCENARIOS {
        let mut rng = StdRng::seed_from_u64(seed);
        let steps = rng.gen_range(5..40);
        let records = random_scenario(&mut rng, steps);
        let expected = resolve(&records);
        accepted_removals += expected.kicked.len();

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
    println!(
        "{SCENARIOS} scenarios x {DELIVERIES_PER_SCENARIO} delivery orders converged, \
         incremental == resolve(held) after every arrival; {accepted_removals} kicks accepted \
         overall; {} records joined, {} applied incrementally, {} full re-resolves ({} of them for voids)",
        totals.joined, totals.incremental, totals.rebuilds, totals.rebuilds_for_voids
    );
}

fn apply_all(records: &[Record]) -> (Resolver, Duration) {
    let mut resolver = Resolver::new();
    let start = Instant::now();
    for record in records {
        resolver.insert(record.clone());
    }
    (resolver, start.elapsed())
}

#[test]
fn incremental_vs_full_on_a_large_control_log() {
    let mut rng = StdRng::seed_from_u64(42);
    let written = random_scenario(&mut rng, 7_150);
    let n = written.len();

    let start = Instant::now();
    let expected = resolve(&written);
    let full = start.elapsed();
    println!(
        "full resolve of {n} control records ({} accepted): {full:?}",
        expected.accepted.len()
    );

    // A live peer: records arrive in the order they were written.
    let (mut resolver, _) = apply_all(&written[..n - 1]);
    let start = Instant::now();
    resolver.insert(written[n - 1].clone());
    let last = start.elapsed();
    assert_eq!(resolver.state(), &expected);
    println!("incremental: applying record #{n} to the other {} took {last:?}", n - 1);

    let (resolver, total) = apply_all(&written);
    assert_eq!(resolver.state(), &expected);
    println!(
        "incremental, written order: all {n} records in {total:?} ({:?} per record; {})",
        total / n as u32,
        describe(resolver.stats())
    );

    // Catching up after being offline: everything arrives shuffled.
    let mut shuffled = written.clone();
    shuffled.shuffle(&mut rng);
    let (resolver, total) = apply_all(&shuffled);
    assert_eq!(resolver.state(), &expected);
    println!(
        "incremental, random order: all {n} records in {total:?} ({})",
        describe(resolver.stats())
    );
}

fn describe(stats: Stats) -> String {
    format!(
        "{} joined, {} incremental, {} full re-resolves of which {} for voids",
        stats.joined, stats.incremental, stats.rebuilds, stats.rebuilds_for_voids
    )
}
