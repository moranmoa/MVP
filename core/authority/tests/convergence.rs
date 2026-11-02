//! Spike experiment 3: peers that receive the same records in any order must
//! end up with the same server state.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use authority::{resolve, Action, Actor, Record, RecordId, Role, State};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

const ACTORS: u32 = 5;
const SCENARIOS: u64 = 10_000;
const DELIVERIES_PER_SCENARIO: usize = 3;

fn random_action(rng: &mut StdRng, author: Actor) -> Action {
    let target = rng.gen_range(0..ACTORS);
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
        known.iter().flat_map(|id| all[id].deps.iter().copied()).collect();
    known.iter().copied().filter(|id| !referenced.contains(id)).collect()
}

/// Actors write records against their own partial view of the log and
/// occasionally sync with each other, so plenty of records are concurrent.
fn random_scenario(rng: &mut StdRng, steps: usize) -> Vec<Record> {
    let genesis = Record::new(0, 0, vec![], Action::Genesis);
    let mut all = BTreeMap::from([(genesis.id, genesis.clone())]);
    let mut known: Vec<BTreeSet<RecordId>> =
        (0..ACTORS).map(|_| BTreeSet::from([genesis.id])).collect();
    let mut seq = vec![1u64; ACTORS as usize];

    for _ in 0..steps {
        let actor = rng.gen_range(0..ACTORS) as usize;
        if rng.gen_bool(0.3) {
            let other = rng.gen_range(0..ACTORS) as usize;
            let theirs = known[other].clone();
            known[actor].extend(theirs);
        } else {
            let deps = heads(&known[actor], &all);
            let action = random_action(rng, actor as Actor);
            let record = Record::new(actor as Actor, seq[actor], deps, action);
            seq[actor] += 1;
            known[actor].insert(record.id);
            all.insert(record.id, record);
        }
    }
    all.into_values().collect()
}

/// A peer receives records one by one in random order and recomputes its
/// state after each arrival, the way a live client would.
fn deliver_in_random_order(records: &[Record], rng: &mut StdRng) -> State {
    let mut arriving = records.to_vec();
    arriving.shuffle(rng);
    let mut held = Vec::with_capacity(arriving.len());
    let mut state = State::default();
    for record in arriving {
        held.push(record);
        state = resolve(&held);
    }
    state
}

#[test]
fn all_delivery_orders_converge() {
    let mut accepted_removals = 0usize;
    for seed in 0..SCENARIOS {
        let mut rng = StdRng::seed_from_u64(seed);
        let steps = rng.gen_range(5..40);
        let records = random_scenario(&mut rng, steps);
        let expected = resolve(&records);
        accepted_removals += expected.kicked.len();

        for _ in 0..DELIVERIES_PER_SCENARIO {
            let state = deliver_in_random_order(&records, &mut rng);
            assert_eq!(state, expected, "diverged on seed {seed}");
        }
    }
    // Guard against a generator that never exercises the hard cases.
    assert!(accepted_removals > 0, "no scenario accepted a kick");
    println!(
        "{SCENARIOS} scenarios x {DELIVERIES_PER_SCENARIO} delivery orders converged; \
         {accepted_removals} kicks accepted overall"
    );
}

#[test]
fn resolve_time_for_a_large_control_log() {
    let mut rng = StdRng::seed_from_u64(42);
    let records = random_scenario(&mut rng, 3_000);
    let start = Instant::now();
    let state = resolve(&records);
    println!(
        "resolved {} control records ({} accepted) in {:?}",
        records.len(),
        state.accepted.len(),
        start.elapsed()
    );
}
