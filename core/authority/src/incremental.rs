//! Incremental resolution: apply records as they arrive without refolding
//! the whole log, and still end up exactly where [`resolve`](crate::resolve)
//! would.
//!
//! The linear order is a min-ID topological sort, so a newly complete record
//! slots in at the first position after its deps whose record has a larger
//! ID; nothing else moves. The record is judged against the role history just
//! before that position, which the resolver keeps per actor.
//!
//! - A post never changes roles, so applying it is all there is.
//! - A grant, revoke or kick changes only its target's role. When it lands
//!   before records that are by or about that target, only those records are
//!   judged again, and so on for any role they in turn change. Nothing else in
//!   the log is touched.
//! - A join changes only its author's membership, and is handled like a
//!   grant. Invites and revoke-invites change no roles. Whether a join is
//!   accepted also depends on its invite, the invite's revokes and its other
//!   joins (the use limit); when one of those could judge differently, or a
//!   later join or revoke of the same invite is already applied, the resolver
//!   re-resolves.
//! - A removal that voids records already applied falls back to a full
//!   re-resolve, and so does a change in which removals were accepted.
//!
//! The reference folds repeatedly until the voided set stops growing, and
//! records voided in an earlier pass stay voided. The resolver keeps what it
//! needs of that history (who was ever removed, whose role history differed
//! between passes) so it can judge a new record the way the reference would,
//! and falls back to a full re-resolve when the earlier passes matter.

use std::collections::{BTreeMap, HashMap, HashSet};

use records::{Record, RecordId, Scope, ServerId};

use crate::{apply, full_run, permitted, Action, Actor, Invite, Role, State, View, Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Records that joined the log (all their deps had arrived).
    pub joined: usize,
    /// Of those, records applied without a full re-resolve.
    pub incremental: usize,
    /// Full re-resolves the resolver fell back to.
    pub rebuilds: usize,
    /// Of those, the ones caused by a removal voiding applied records.
    pub rebuilds_for_voids: usize,
}

/// Why a record couldn't be applied incrementally.
enum Fallback {
    Voids,
    Other,
}

/// What [`Resolver::insert`] did with a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inserted {
    /// It joined the log, along with any waiting records it unblocked.
    Joined,
    /// Some of its deps haven't arrived; it waits for them. If the pending
    /// queue was full, the record that had waited longest made room and is
    /// handed back: request its deps (or the record) again later.
    Pending { evicted: Option<Box<Record>> },
    /// A duplicate, another server's record, or not a control-log record.
    Ignored,
}

/// Default for how many records may wait on missing deps at once. Anyone can
/// sign records that point at deps which will never arrive, so the queue is
/// bounded (at most about 26 MB of control records), and when it is full the
/// oldest waiting record is evicted: junk ages out instead of holding the
/// queue.
pub const MAX_PENDING: usize = 4096;

/// A record waiting on deps.
struct Waiting {
    /// How many of its deps haven't joined.
    missing: usize,
    /// Arrival number, for evicting the oldest.
    arrival: u64,
}

/// Holds one server's control log and its resolved state, updated as records
/// arrive in any order.
pub struct Resolver {
    /// The one server whose records this resolver accepts.
    server: ServerId,
    /// Most records that may wait on missing deps.
    pending_limit: usize,
    /// Every control record received, whether or not its deps have arrived.
    known: HashMap<RecordId, Record>,
    /// Records still waiting on deps.
    missing: HashMap<RecordId, Waiting>,
    /// Missing dep -> the records waiting on it.
    waiting: HashMap<RecordId, Vec<RecordId>>,
    /// Waiting records by arrival, oldest first.
    arrivals: BTreeMap<u64, RecordId>,
    next_arrival: u64,

    /// Records that joined the log, in the reference's linear order.
    order: Vec<RecordId>,
    pos: HashMap<RecordId, usize>,
    actions: HashMap<RecordId, Option<Action>>,
    by_author: HashMap<Actor, Vec<RecordId>>,
    /// Actor -> records by or about them, in linear order.
    touch: HashMap<Actor, Vec<RecordId>>,
    /// Invite -> the joins and the revoke-invites naming it, in linear order,
    /// accepted or not.
    invite_joins: HashMap<RecordId, Vec<RecordId>>,
    invite_revokes: HashMap<RecordId, Vec<RecordId>>,

    // What the reference run would hold for the joined records.
    state: State,
    writes: HashMap<Actor, Vec<Write>>,
    genesis: Option<RecordId>,
    excluded: HashSet<RecordId>,
    kick_targets: HashSet<Actor>,
    revoke_targets: HashSet<Actor>,
    unsettled: HashSet<Actor>,
    owner_unsettled: bool,

    /// A record couldn't be applied incrementally; re-resolve once the
    /// current arrival and the records it unblocked have joined.
    stale: Option<Fallback>,
    stats: Stats,
}

impl Resolver {
    /// A resolver for `server` (see [`ServerId::derive`]).
    pub fn new(server: ServerId) -> Self {
        Self::with_pending_limit(server, MAX_PENDING)
    }

    /// Like [`Self::new`], with room for `limit` (at least 1) records waiting
    /// on deps.
    pub fn with_pending_limit(server: ServerId, limit: usize) -> Self {
        Resolver {
            server,
            pending_limit: limit.max(1),
            known: HashMap::new(),
            missing: HashMap::new(),
            waiting: HashMap::new(),
            arrivals: BTreeMap::new(),
            next_arrival: 0,
            order: Vec::new(),
            pos: HashMap::new(),
            actions: HashMap::new(),
            by_author: HashMap::new(),
            touch: HashMap::new(),
            invite_joins: HashMap::new(),
            invite_revokes: HashMap::new(),
            state: State::default(),
            writes: HashMap::new(),
            genesis: None,
            excluded: HashSet::new(),
            kick_targets: HashSet::new(),
            revoke_targets: HashSet::new(),
            unsettled: HashSet::new(),
            owner_unsettled: false,
            stale: None,
            stats: Stats::default(),
        }
    }

    /// How many records are waiting on missing deps.
    pub fn pending(&self) -> usize {
        self.missing.len()
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Adds an arriving record. Duplicates, other servers' records and
    /// records outside the control log are ignored; records with missing
    /// deps wait for them; when the pending queue is full, the oldest waiting
    /// record is evicted and returned.
    pub fn insert(&mut self, record: Record) -> Inserted {
        let id = record.id();
        if record.server() != self.server
            || record.scope() != Scope::Control
            || self.known.contains_key(&id)
        {
            return Inserted::Ignored;
        }
        let missing: Vec<RecordId> =
            record.deps().iter().filter(|dep| !self.pos.contains_key(*dep)).copied().collect();
        if !missing.is_empty() {
            let evicted =
                if self.missing.len() >= self.pending_limit { self.evict_oldest() } else { None };
            for dep in &missing {
                self.waiting.entry(*dep).or_default().push(id);
            }
            let arrival = self.next_arrival;
            self.next_arrival += 1;
            self.arrivals.insert(arrival, id);
            self.missing.insert(id, Waiting { missing: missing.len(), arrival });
            self.known.insert(id, record);
            return Inserted::Pending { evicted: evicted.map(Box::new) };
        }
        self.known.insert(id, record);

        let mut ready = vec![id];
        while let Some(id) = ready.pop() {
            self.join(id);
            for waiter in self.waiting.remove(&id).unwrap_or_default() {
                let entry = self.missing.get_mut(&waiter).expect("waiter is pending");
                entry.missing -= 1;
                if entry.missing == 0 {
                    let arrival = entry.arrival;
                    self.missing.remove(&waiter);
                    self.arrivals.remove(&arrival);
                    ready.push(waiter);
                }
            }
        }
        if let Some(reason) = self.stale.take() {
            self.rebuild(reason);
        }
        Inserted::Joined
    }

    /// Forgets the record that has waited longest, so a flood of records
    /// whose deps never arrive ages out instead of holding the queue.
    fn evict_oldest(&mut self) -> Option<Record> {
        let (_, id) = self.arrivals.pop_first()?;
        self.missing.remove(&id);
        let record = self.known.remove(&id).expect("waiting records are known");
        for dep in record.deps() {
            if let Some(waiters) = self.waiting.get_mut(dep) {
                waiters.retain(|waiter| *waiter != id);
                if waiters.is_empty() {
                    self.waiting.remove(dep);
                }
            }
        }
        Some(record)
    }

    /// Puts a record whose deps have all joined into the log.
    fn join(&mut self, id: RecordId) {
        let record = &self.known[&id];
        let author = record.author();
        let action = Action::from_record(record);
        let at = self.insertion_point(record);

        self.order.insert(at, id);
        for (i, later) in self.order.iter().enumerate().skip(at) {
            self.pos.insert(*later, i);
        }
        self.by_author.entry(author).or_default().push(id);
        let target = action.as_ref().and_then(Action::target);
        for actor in std::iter::once(author).chain(target.filter(|t| *t != author)) {
            let list = self.touch.entry(actor).or_default();
            let pos = &self.pos;
            let i = list.partition_point(|other| pos[other] < at);
            list.insert(i, id);
        }
        let named = match &action {
            Some(Action::Join { invite }) => Some((&mut self.invite_joins, *invite)),
            Some(Action::RevokeInvite { invite }) => Some((&mut self.invite_revokes, *invite)),
            _ => None,
        };
        if let Some((lists, invite)) = named {
            let list = lists.entry(invite).or_default();
            let pos = &self.pos;
            let i = list.partition_point(|other| pos[other] < at);
            list.insert(i, id);
        }
        self.actions.insert(id, action);

        self.stats.joined += 1;
        if self.stale.is_none() {
            match self.apply_at(id, at) {
                Ok(()) => self.stats.incremental += 1,
                Err(reason) => self.stale = Some(reason),
            }
        }
    }

    /// Where the reference's min-ID topological sort puts a new record:
    /// nothing before its last dep changes, and from there it goes ahead of
    /// the first record with a larger ID.
    fn insertion_point(&self, record: &Record) -> usize {
        let start = record.deps().iter().map(|dep| self.pos[dep] + 1).max().unwrap_or(0);
        self.order[start..]
            .iter()
            .position(|other| *other > record.id())
            .map_or(self.order.len(), |offset| start + offset)
    }

    fn is_unsettled(&self, actor: Actor) -> bool {
        self.unsettled.contains(&actor)
    }

    /// Applies the new record at position `at` the way the reference would.
    fn apply_at(&mut self, id: RecordId, at: usize) -> Result<(), Fallback> {
        let author = self.known[&id].author();
        let Some(action) = self.actions[&id].clone() else {
            // Never accepted; it can still be voided like any record.
            if self.kick_targets.contains(&author) {
                self.excluded.insert(id);
            }
            return Ok(());
        };
        // An accepted removal of the author in any pass voids this record:
        // it is new, so it can't be an ancestor of anything already here.
        let voided = self.kick_targets.contains(&author)
            || (action.is_privileged() && self.revoke_targets.contains(&author));
        let allowed = permitted(&self.at(at), &self.known[&id], &action);

        let target = match action {
            Action::Genesis => {
                // Only a genesis ordered after the accepted one is harmless.
                if self.genesis.is_none_or(|genesis| self.pos[&genesis] >= at) {
                    return Err(Fallback::Other);
                }
                if voided {
                    self.excluded.insert(id);
                }
                return Ok(());
            }
            Action::Post { .. } => {
                // Posts change no roles, so other passes don't matter.
                if voided {
                    self.excluded.insert(id);
                } else if allowed {
                    self.add_post(id);
                }
                return Ok(());
            }
            Action::Invite { .. } | Action::RevokeInvite { .. } => {
                return self.apply_invite_record(id, at, &action, voided, allowed);
            }
            Action::Join { invite } => {
                // Its own acceptance may differ between passes. That only
                // matters through the joiner's later records, so if there
                // are none yet, mark the joiner unsettled and judge it by
                // the final pass.
                if self.join_is_shaky(id, at, invite)? || self.is_unsettled(author) {
                    if self.owner_unsettled || self.touch[&author].last() != Some(&id) {
                        return Err(Fallback::Other);
                    }
                    self.unsettled.insert(author);
                }
                author
            }
            Action::Grant { target, .. } | Action::Revoke { target } | Action::Kick { target } => {
                target
            }
        };

        // From here on the record is judged the same way in every pass.
        let shaky_join = matches!(action, Action::Join { .. }) && self.is_unsettled(author);
        if !shaky_join
            && (self.owner_unsettled || self.is_unsettled(author) || self.is_unsettled(target))
        {
            return Err(Fallback::Other);
        }
        let removal = action.is_removal();
        if voided {
            // Voided only after the passes that accepted it.
            if allowed {
                if removal || self.touch[&target].last() != Some(&id) {
                    return Err(Fallback::Other);
                }
                self.unsettled.insert(target);
            }
            self.excluded.insert(id);
            return Ok(());
        }
        if !allowed {
            return Ok(());
        }
        if removal {
            let kick = matches!(action, Action::Kick { .. });
            let voids = self.concurrent_by(id, target, !kick);
            if voids.iter().any(|v| !self.excluded.contains(v)) {
                return Err(Fallback::Voids);
            }
            if kick {
                self.kick_targets.insert(target);
            } else {
                self.revoke_targets.insert(target);
            }
        }
        self.accept_write(id, &action);
        if let Action::Join { invite } = action {
            self.refresh_invite(invite);
        }
        self.propagate(at, target)
    }

    /// A join is judged against its invite, the invite's revokes and the
    /// invite's earlier joins. Returns whether any of those could have been
    /// judged differently in an earlier pass of the reference. Falls back
    /// when this join lands ahead of another join it competes with for a use.
    fn join_is_shaky(&self, id: RecordId, at: usize, invite: RecordId) -> Result<bool, Fallback> {
        let record = &self.known[&id];
        // Without its invite in deps the join is refused in every pass.
        if !record.deps().contains(&invite) {
            return Ok(false);
        }
        let Some(Action::Invite { max_uses, .. }) = self.actions[&invite] else {
            return Ok(false);
        };
        let shaky = |other: &RecordId| {
            self.excluded.contains(other) || self.is_unsettled(self.known[other].author())
        };
        let others = || self.invite_joins.get(&invite).into_iter().flatten();
        if max_uses != 0 && others().any(|other| *other != id && self.pos[other] > at) {
            return Err(Fallback::Other);
        }
        Ok(shaky(&invite)
            || self.invite_revokes.get(&invite).into_iter().flatten().any(shaky)
            || (max_uses != 0 && others().any(|other| *other != id && shaky(other))))
    }

    /// Applies a new invite or revoke-invite. Neither changes a role; what
    /// they change is which joins are accepted. An invite can't have joins
    /// or revokes yet (they depend on it). A revoke-invite that lands ahead
    /// of already-applied joins or revokes of the same invite falls back.
    /// If either could be judged differently in an earlier pass, a later
    /// join through the invite finds out from `excluded` and `unsettled`
    /// (see [`Self::join_is_shaky`]).
    fn apply_invite_record(
        &mut self,
        id: RecordId,
        at: usize,
        action: &Action,
        voided: bool,
        allowed: bool,
    ) -> Result<(), Fallback> {
        if self.owner_unsettled {
            return Err(Fallback::Other);
        }
        let invite = match action {
            Action::RevokeInvite { invite } => {
                let invite = *invite;
                let later = |list: Option<&Vec<RecordId>>| {
                    list.into_iter().flatten().any(|other| *other != id && self.pos[other] > at)
                };
                if later(self.invite_joins.get(&invite)) || later(self.invite_revokes.get(&invite))
                {
                    return Err(Fallback::Other);
                }
                invite
            }
            _ => id,
        };
        if voided {
            self.excluded.insert(id);
        } else if allowed {
            self.state.accepted.insert(id);
            self.refresh_invite(invite);
        }
        Ok(())
    }

    /// Recomputes an invite's entry in the state from its accepted joins and
    /// revokes.
    fn refresh_invite(&mut self, invite: RecordId) {
        let Some(Some(Action::Invite { expires_ms, max_uses })) = self.actions.get(&invite) else {
            return;
        };
        if !self.state.accepted.contains(&invite) {
            self.state.invites.remove(&invite);
            return;
        }
        let accepted = |list: Option<&Vec<RecordId>>| {
            list.into_iter().flatten().filter(|id| self.state.accepted.contains(*id)).count()
        };
        let entry = Invite {
            author: self.known[&invite].author(),
            expires_ms: *expires_ms,
            max_uses: *max_uses,
            uses: u32::try_from(accepted(self.invite_joins.get(&invite))).unwrap_or(u32::MAX),
            revoked: accepted(self.invite_revokes.get(&invite)) > 0,
        };
        self.state.invites.insert(invite, entry);
    }

    /// Judges again the records after `at` that are by or about an actor
    /// whose role history just changed, until no more roles change.
    fn propagate(&mut self, at: usize, changed: Actor) -> Result<(), Fallback> {
        let mut dirty = HashSet::from([changed]);
        for i in at + 1..self.order.len() {
            let id = self.order[i];
            let Some(action) = &self.actions[&id] else {
                continue;
            };
            let author = self.known[&id].author();
            // Whose role history the record is judged against.
            let target = action.target();
            let inviter =
                action.invite().and_then(|invite| self.known.get(&invite)).map(|r| r.author());
            let involved = [Some(author), target, inviter];
            if !involved.iter().flatten().any(|actor| dirty.contains(actor)) {
                continue;
            }
            let action = action.clone();
            match action {
                // A later genesis stays rejected: the owner can't change.
                Action::Genesis => continue,
                Action::Post { .. } if self.excluded.contains(&id) => continue,
                _ if self.excluded.contains(&id) => return Err(Fallback::Other),
                _ => {}
            }
            if !matches!(action, Action::Post { .. })
                && involved.iter().flatten().any(|actor| self.is_unsettled(*actor))
            {
                return Err(Fallback::Other);
            }
            let was = self.state.accepted.contains(&id);
            let now = permitted(&self.at(i), &self.known[&id], &action);
            if was == now {
                continue;
            }
            match action {
                Action::Post { .. } => {
                    if now {
                        self.add_post(id);
                    } else {
                        self.state.posts.retain(|post| *post != id);
                        self.state.accepted.remove(&id);
                    }
                }
                Action::Grant { target, .. } => {
                    if now {
                        self.accept_write(id, &action);
                    } else {
                        self.state.accepted.remove(&id);
                        self.writes
                            .get_mut(&target)
                            .expect("accepted grant")
                            .retain(|w| w.record != id);
                    }
                    dirty.insert(target);
                }
                // Removals accepted differently void different records.
                Action::Revoke { .. } | Action::Kick { .. } => return Err(Fallback::Voids),
                // These change which joins are accepted, and joins compete
                // for an invite's uses.
                Action::Invite { .. } | Action::RevokeInvite { .. } | Action::Join { .. } => {
                    return Err(Fallback::Other)
                }
                Action::Genesis => unreachable!("skipped above"),
            }
        }

        // Each dirty actor's final role is the last write it received.
        for actor in dirty {
            let last = self.writes.get(&actor).and_then(|writes| writes.last());
            match last.map_or(Role::Member, |w| w.role) {
                Role::Member => self.state.roles.remove(&actor),
                role => self.state.roles.insert(actor, role),
            };
            if last.is_some_and(|w| w.kicked) {
                self.state.kicked.insert(actor);
            } else {
                self.state.kicked.remove(&actor);
            }
            if last.is_some_and(|w| w.member) || self.state.owner == Some(actor) {
                self.state.members.insert(actor);
            } else {
                self.state.members.remove(&actor);
            }
        }
        Ok(())
    }

    fn add_post(&mut self, id: RecordId) {
        let pos = &self.pos;
        let i = self.state.posts.partition_point(|post| pos[post] < pos[&id]);
        self.state.posts.insert(i, id);
        self.state.accepted.insert(id);
    }

    /// Records an accepted grant or removal in its target's role history.
    /// The target's final role is settled by [`Self::propagate`].
    fn accept_write(&mut self, id: RecordId, action: &Action) {
        self.state.accepted.insert(id);
        let mut scratch = State::default();
        if let Some((target, write)) = apply(&mut scratch, &self.known[&id], action) {
            let writes = self.writes.entry(target).or_default();
            let pos = &self.pos;
            let i = writes.partition_point(|w| pos[&w.record] < pos[&id]);
            writes.insert(i, write);
        }
    }

    /// Records by `target` that the removal `id` hadn't seen.
    fn concurrent_by(&self, id: RecordId, target: Actor, privileged_only: bool) -> Vec<RecordId> {
        let mut seen = HashSet::new();
        let mut stack: Vec<RecordId> = self.known[&id].deps().to_vec();
        while let Some(next) = stack.pop() {
            if seen.insert(next) {
                stack.extend_from_slice(self.known[&next].deps());
            }
        }
        self.by_author
            .get(&target)
            .into_iter()
            .flatten()
            .copied()
            .filter(|other| *other != id && !seen.contains(other))
            .filter(|other| {
                !privileged_only || self.actions[other].as_ref().is_some_and(Action::is_privileged)
            })
            .collect()
    }

    fn at(&self, at: usize) -> AtPosition<'_> {
        AtPosition { resolver: self, at }
    }

    fn rebuild(&mut self, reason: Fallback) {
        let run = full_run(self.server, self.order.iter().map(|id| &self.known[id]));
        if run.order != self.order {
            // Can't happen unless `insertion_point` has a bug. Take the
            // reference's order rather than panic on records a peer sent.
            debug_assert!(false, "incremental order diverged from the reference");
            self.order = run.order;
            self.pos = self.order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            let pos = &self.pos;
            for list in self
                .touch
                .values_mut()
                .chain(self.invite_joins.values_mut())
                .chain(self.invite_revokes.values_mut())
            {
                list.sort_by_key(|id| pos[id]);
            }
        }
        self.state = run.state;
        self.writes = run.writes;
        self.genesis = run.genesis;
        self.excluded = run.excluded;
        self.kick_targets = run.kick_targets;
        self.revoke_targets = run.revoke_targets;
        self.unsettled = run.unsettled;
        self.owner_unsettled = run.owner_unsettled;
        self.stats.rebuilds += 1;
        if let Fallback::Voids = reason {
            self.stats.rebuilds_for_voids += 1;
        }
    }
}

/// The final pass's roles just before position `at` in the linear order.
struct AtPosition<'a> {
    resolver: &'a Resolver,
    at: usize,
}

impl AtPosition<'_> {
    fn last_write(&self, actor: Actor) -> Option<&Write> {
        let writes = self.resolver.writes.get(&actor)?;
        let before = writes.partition_point(|w| self.resolver.pos[&w.record] < self.at);
        before.checked_sub(1).map(|i| &writes[i])
    }
}

impl View for AtPosition<'_> {
    fn owner(&self) -> Option<Actor> {
        let r = self.resolver;
        r.genesis.filter(|genesis| r.pos[genesis] < self.at).and(r.state.owner)
    }

    fn assigned_role(&self, actor: Actor) -> Role {
        self.last_write(actor).map_or(Role::Member, |w| w.role)
    }

    fn is_kicked(&self, actor: Actor) -> bool {
        self.last_write(actor).is_some_and(|w| w.kicked)
    }

    fn has_joined(&self, actor: Actor) -> bool {
        self.last_write(actor).is_some_and(|w| w.member)
    }

    /// The invite with the joins and revokes accepted before this position.
    fn invite(&self, id: RecordId) -> Option<Invite> {
        let r = self.resolver;
        if !r.state.accepted.contains(&id) || r.pos.get(&id).is_none_or(|pos| *pos >= self.at) {
            return None;
        }
        let Some(Some(Action::Invite { expires_ms, max_uses })) = r.actions.get(&id) else {
            return None;
        };
        let before = |list: Option<&'_ Vec<RecordId>>| {
            list.into_iter()
                .flatten()
                .filter(|other| r.pos[*other] < self.at && r.state.accepted.contains(*other))
                .count()
        };
        Some(Invite {
            author: r.known[&id].author(),
            expires_ms: *expires_ms,
            max_uses: *max_uses,
            uses: u32::try_from(before(r.invite_joins.get(&id))).unwrap_or(u32::MAX),
            revoked: before(r.invite_revokes.get(&id)) > 0,
        })
    }
}
