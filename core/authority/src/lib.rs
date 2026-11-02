//! Deterministic resolution of a server's control log (spike, experiment 3).
//!
//! Every peer runs [`resolve`] on the set of control records it holds. The
//! result depends only on that set, never on the order records arrived in, so
//! peers holding the same records always agree on the server state.
//!
//! Rules covered by the spike:
//! - The genesis author is the owner and can't be demoted or kicked.
//! - Rank: owner > admin > moderator > member. An actor can only grant a role
//!   below its own, and only revoke or kick someone ranked below it.
//! - A revoke voids the target's privileged actions that are concurrent with
//!   it (not causal ancestors of it). A kick voids all of the target's
//!   concurrent records. This blocks "backdating": signing new records whose
//!   deps point at state from before the removal.
//! - Ties between concurrent records are broken by the record ID.
//!
//! Signatures, encryption and timestamps are left out: they don't affect
//! ordering, which is what this spike tests.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

pub type Actor = u32;
pub type RecordId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Member,
    Moderator,
    Admin,
    Owner,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Genesis,
    Grant { target: Actor, role: Role },
    Revoke { target: Actor },
    Kick { target: Actor },
    Post { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: RecordId,
    pub author: Actor,
    pub seq: u64,
    /// IDs of the records the author had seen (the heads of its log view).
    pub deps: Vec<RecordId>,
    pub action: Action,
}

impl Record {
    pub fn new(author: Actor, seq: u64, mut deps: Vec<RecordId>, action: Action) -> Self {
        deps.sort_unstable();
        deps.dedup();
        let id = record_id(author, seq, &deps, &action);
        Record { id, author, seq, deps, action }
    }

    fn is_privileged(&self) -> bool {
        matches!(
            self.action,
            Action::Grant { .. } | Action::Revoke { .. } | Action::Kick { .. }
        )
    }
}

/// FNV-1a over the record's content. Stands in for the content hash a real
/// record would carry; it only has to be deterministic here.
fn record_id(author: Actor, seq: u64, deps: &[RecordId], action: &Action) -> RecordId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(&author.to_le_bytes());
    feed(&seq.to_le_bytes());
    for dep in deps {
        feed(&dep.to_le_bytes());
    }
    feed(format!("{action:?}").as_bytes());
    hash
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub owner: Option<Actor>,
    pub roles: BTreeMap<Actor, Role>,
    pub kicked: BTreeSet<Actor>,
    /// Accepted posts, in display order.
    pub posts: Vec<RecordId>,
    pub accepted: BTreeSet<RecordId>,
}

impl State {
    pub fn role(&self, actor: Actor) -> Role {
        if self.owner == Some(actor) {
            Role::Owner
        } else {
            self.roles.get(&actor).copied().unwrap_or(Role::Member)
        }
    }
}

/// Computes the server state from a set of control records. Records whose
/// deps aren't all in the set are ignored.
pub fn resolve(records: &[Record]) -> State {
    let by_id = complete_records(records);
    let order = linear_order(&by_id);
    let ancestors = ancestor_sets(&by_id, &order);

    // A valid removal can void records that came earlier in the linear
    // order, so fold again until no new records get voided. The excluded set
    // only grows, so this terminates.
    let mut excluded = BTreeSet::new();
    loop {
        let (state, voided) = fold(&by_id, &order, &ancestors, &excluded);
        if voided.is_subset(&excluded) {
            return state;
        }
        excluded.extend(voided);
    }
}

/// Keeps only records whose whole dependency chain is present.
fn complete_records(records: &[Record]) -> BTreeMap<RecordId, &Record> {
    let mut by_id: BTreeMap<RecordId, &Record> = records.iter().map(|r| (r.id, r)).collect();
    loop {
        let missing: Vec<RecordId> = by_id
            .values()
            .filter(|r| r.deps.iter().any(|dep| !by_id.contains_key(dep)))
            .map(|r| r.id)
            .collect();
        if missing.is_empty() {
            return by_id;
        }
        for id in missing {
            by_id.remove(&id);
        }
    }
}

/// Topological order of the record graph; concurrent records go in ID order.
fn linear_order(by_id: &BTreeMap<RecordId, &Record>) -> Vec<RecordId> {
    let mut pending_deps: BTreeMap<RecordId, usize> =
        by_id.values().map(|r| (r.id, r.deps.len())).collect();
    let mut dependents: BTreeMap<RecordId, Vec<RecordId>> = BTreeMap::new();
    for record in by_id.values() {
        for dep in &record.deps {
            dependents.entry(*dep).or_default().push(record.id);
        }
    }

    let mut ready: BinaryHeap<Reverse<RecordId>> = pending_deps
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| Reverse(*id))
        .collect();
    let mut order = Vec::with_capacity(by_id.len());
    while let Some(Reverse(id)) = ready.pop() {
        order.push(id);
        for next in dependents.get(&id).into_iter().flatten() {
            let count = pending_deps.get_mut(next).expect("dependent is in the set");
            *count -= 1;
            if *count == 0 {
                ready.push(Reverse(*next));
            }
        }
    }
    order
}

fn ancestor_sets(
    by_id: &BTreeMap<RecordId, &Record>,
    order: &[RecordId],
) -> BTreeMap<RecordId, BTreeSet<RecordId>> {
    let mut ancestors: BTreeMap<RecordId, BTreeSet<RecordId>> = BTreeMap::new();
    for id in order {
        let mut set = BTreeSet::new();
        for dep in &by_id[id].deps {
            set.insert(*dep);
            set.extend(ancestors[dep].iter().copied());
        }
        ancestors.insert(*id, set);
    }
    ancestors
}

fn fold(
    by_id: &BTreeMap<RecordId, &Record>,
    order: &[RecordId],
    ancestors: &BTreeMap<RecordId, BTreeSet<RecordId>>,
    excluded: &BTreeSet<RecordId>,
) -> (State, BTreeSet<RecordId>) {
    let mut state = State::default();
    let mut voided = BTreeSet::new();

    let concurrent_by = |removal: &Record, target: Actor, privileged_only: bool| {
        by_id
            .values()
            .filter(|r| r.author == target && r.id != removal.id)
            .filter(|r| !ancestors[&removal.id].contains(&r.id))
            .filter(|r| !privileged_only || r.is_privileged())
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };

    for id in order {
        if excluded.contains(id) {
            continue;
        }
        let record = by_id[id];
        if state.kicked.contains(&record.author) {
            continue;
        }
        let author_role = state.role(record.author);
        let accepted = match &record.action {
            Action::Genesis => {
                let ok = state.owner.is_none() && record.deps.is_empty();
                if ok {
                    state.owner = Some(record.author);
                }
                ok
            }
            _ if state.owner.is_none() => false,
            Action::Grant { target, role } => {
                let ok = *role < Role::Owner
                    && author_role > *role
                    && author_role > state.role(*target)
                    && !state.kicked.contains(target);
                if ok {
                    if *role == Role::Member {
                        state.roles.remove(target);
                    } else {
                        state.roles.insert(*target, *role);
                    }
                }
                ok
            }
            Action::Revoke { target } => {
                let ok = author_role > state.role(*target) && state.role(*target) > Role::Member;
                if ok {
                    state.roles.remove(target);
                    voided.extend(concurrent_by(record, *target, true));
                }
                ok
            }
            Action::Kick { target } => {
                let ok = author_role >= Role::Moderator && author_role > state.role(*target);
                if ok {
                    state.roles.remove(target);
                    state.kicked.insert(*target);
                    voided.extend(concurrent_by(record, *target, false));
                }
                ok
            }
            Action::Post { .. } => {
                state.posts.push(record.id);
                true
            }
        };
        if accepted {
            state.accepted.insert(record.id);
        }
    }
    (state, voided)
}
