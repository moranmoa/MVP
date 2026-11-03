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
//!   joins (the use limit). When one of those may be judged differently in
//!   an earlier pass and the joiner has no later records yet, the joiner is
//!   marked unsettled; otherwise the resolver re-resolves.
//! - A revoke-invite voids the joins through its invite that it hadn't
//!   seen, like a removal. When it may have been accepted in an earlier pass
//!   only, a later join through the invite is only re-resolved if the final
//!   pass accepts it.
//! - Once every pass accepted the same kick of an actor, their history is
//!   the same in all passes from there on, so records after that kick are
//!   applied incrementally even if the actor's earlier history is unsettled.
//! - A removal that voids records already applied falls back to a full
//!   re-resolve, and so does a change in which removals were accepted.
//!
//! The reference folds repeatedly until the voided set stops growing, and
//! records voided in an earlier pass stay voided. The resolver keeps what it
//! needs of that history (who was ever removed, whose role history differed
//! between passes) so it can judge a new record the way the reference would,
//! and falls back to a full re-resolve when the earlier passes matter.

use std::collections::{BTreeMap, HashMap, HashSet};

use records::{ChannelId, Record, RecordId, Scope, ServerId};

use crate::{apply, full_run, permitted, Action, Actor, Channel, Invite, Role, State, View, Write};

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
    /// Channel -> the records creating it, accepted or not.
    channel_creates: HashMap<ChannelId, Vec<RecordId>>,

    // What the reference run would hold for the joined records.
    state: State,
    writes: HashMap<Actor, Vec<Write>>,
    genesis: Option<RecordId>,
    excluded: HashSet<RecordId>,
    /// Kicks and revokes accepted in any pass, by target.
    kick_targets: HashMap<Actor, Vec<RecordId>>,
    revoke_targets: HashMap<Actor, Vec<RecordId>>,
    revoked_invites: HashSet<RecordId>,
    /// Invites whose revoke-invite may have been accepted in an earlier
    /// pass only; a join through one re-resolves. Resolver-only: a full
    /// re-resolve knows the answer.
    uncertain_invites: HashSet<RecordId>,
    /// Joins through those invites, refused in the final pass, that the
    /// reference may or may not have voided. They count as voided when
    /// deciding whether a record is judged the same in every pass, and as
    /// not voided when deciding whether a void is new.
    maybe_excluded: HashSet<RecordId>,
    unsettled: HashMap<Actor, Option<RecordId>>,
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
            channel_creates: HashMap::new(),
            state: State::default(),
            writes: HashMap::new(),
            genesis: None,
            excluded: HashSet::new(),
            kick_targets: HashMap::new(),
            revoke_targets: HashMap::new(),
            revoked_invites: HashSet::new(),
            uncertain_invites: HashSet::new(),
            maybe_excluded: HashSet::new(),
            unsettled: HashMap::new(),
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
        // Only records that depend on the invite they name can be accepted.
        let depends = action
            .as_ref()
            .and_then(Action::invite)
            .is_some_and(|invite| self.known[&id].deps().contains(&invite));
        let named = match &action {
            Some(Action::Join { invite }) if depends => Some((&mut self.invite_joins, *invite)),
            Some(Action::RevokeInvite { invite }) if depends => {
                Some((&mut self.invite_revokes, *invite))
            }
            _ => None,
        };
        if let Some((lists, invite)) = named {
            let list = lists.entry(invite).or_default();
            let pos = &self.pos;
            let i = list.partition_point(|other| pos[other] < at);
            list.insert(i, id);
        }
        if let Some(Action::ChannelCreate { channel, .. }) = &action {
            self.channel_creates.entry(*channel).or_default().push(id);
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

    fn may_be_excluded(&self, id: &RecordId) -> bool {
        self.excluded.contains(id) || self.maybe_excluded.contains(id)
    }

    /// Whether the actor's history just before position `at` may differ
    /// between the reference's passes.
    fn is_unsettled(&self, actor: Actor, at: usize) -> bool {
        match self.unsettled.get(&actor) {
            None => false,
            Some(None) => true,
            Some(Some(kick)) => self.pos[kick] >= at,
        }
    }

    /// Applies the new record at position `at` the way the reference would.
    fn apply_at(&mut self, id: RecordId, at: usize) -> Result<(), Fallback> {
        let author = self.known[&id].author();
        let Some(action) = self.actions[&id].clone() else {
            // Never accepted; it can still be voided like any record.
            if self.removed_concurrently(&self.kick_targets, author, id) {
                self.excluded.insert(id);
            }
            return Ok(());
        };
        // An accepted removal of the author that this record doesn't descend
        // from, or a revoke of a join's invite, in any pass voids this record:
        // it is new, so it can't be an ancestor of anything already here.
        let voided = self.removed_concurrently(&self.kick_targets, author, id)
            || (action.is_privileged()
                && self.removed_concurrently(&self.revoke_targets, author, id))
            || matches!(&action, Action::Join { invite }
                if self.known[&id].deps().contains(invite) && self.invite_is_dead(*invite));
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
            Action::AgeClaim { .. } => {
                // Changes no roles; whether it counts depends on the author's
                // membership, which must be the same in every pass.
                if self.owner_unsettled || self.is_unsettled(author, at) {
                    return Err(Fallback::Other);
                }
                if voided {
                    self.excluded.insert(id);
                } else if allowed {
                    self.state.accepted.insert(id);
                }
                self.refresh_adult(author);
                return Ok(());
            }
            Action::ChannelCreate { channel, adult, name } => {
                // Only the owner creates channels, and the first create of a
                // channel in linear order wins, like a use of an invite.
                let creates = &self.channel_creates[&channel];
                if self.owner_unsettled || creates.len() > 1 {
                    return Err(Fallback::Other);
                }
                if voided {
                    self.excluded.insert(id);
                } else if allowed {
                    self.state.accepted.insert(id);
                    let entry = Channel { name, adult, created: id };
                    self.state.channels.insert(channel, entry);
                }
                return Ok(());
            }
            Action::Join { invite } => {
                // Whether a revoke of the invite voids it is unknown. That only
                // matters if it is accepted in the final pass.
                if self.uncertain_invites.contains(&invite) {
                    if allowed {
                        return Err(Fallback::Other);
                    }
                    self.maybe_excluded.insert(id);
                }
                // A kicked actor or a member is refused in every pass,
                // whatever the invite.
                let view = self.at(at);
                if !self.owner_unsettled
                    && !self.is_unsettled(author, at)
                    && (view.is_kicked(author) || view.is_member(author))
                {
                    if voided {
                        self.excluded.insert(id);
                    }
                    return Ok(());
                }
                // Its own acceptance may differ between passes. That only
                // matters through the joiner's later records, so if there
                // are none yet, mark the joiner unsettled and judge it by
                // the final pass.
                if self.join_is_shaky(id, at, invite)? || self.is_unsettled(author, at) {
                    if self.owner_unsettled || self.touch[&author].last() != Some(&id) {
                        return Err(Fallback::Other);
                    }
                    self.unsettled.insert(author, None);
                }
                author
            }
            Action::Grant { target, .. }
            | Action::Revoke { target, .. }
            | Action::Kick { target, .. }
            | Action::Unban { target } => target,
        };

        // From here on the record is judged the same way in every pass.
        let shaky_join = matches!(action, Action::Join { .. }) && self.is_unsettled(author, at);
        if !shaky_join
            && (self.owner_unsettled
                || self.is_unsettled(author, at)
                || self.is_unsettled(target, at))
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
                self.unsettled.insert(target, None);
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
            let targets = if kick { &mut self.kick_targets } else { &mut self.revoke_targets };
            targets.entry(target).or_default().push(id);
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
            self.may_be_excluded(other)
                || self.is_unsettled(self.known[other].author(), self.pos[other])
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
    /// or revokes yet (they depend on it); if it could be judged differently
    /// in an earlier pass, a later join through it finds out from `excluded`
    /// and `unsettled` (see [`Self::join_is_shaky`]). A revoke-invite voids
    /// the joins through the invite it hadn't seen, and falls back like a
    /// removal when any of them was applied.
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
        let Action::RevokeInvite { invite } = *action else {
            if voided {
                self.excluded.insert(id);
            } else if allowed {
                self.state.accepted.insert(id);
                self.refresh_invite(id);
            }
            return Ok(());
        };
        // It can't land ahead of a revoke it would replace.
        let revokes = || self.invite_revokes.get(&invite).into_iter().flatten();
        if revokes().any(|other| *other != id && self.pos[other] > at) {
            return Err(Fallback::Other);
        }
        // It voids the same joins in whichever pass accepts it.
        let joins = self.invite_joins.get(&invite).into_iter().flatten();
        let mut applied = joins.filter(|join| !self.excluded.contains(*join)).peekable();
        let voids_applied = applied.peek().is_some() && {
            let ancestors = self.ancestors(id);
            applied.any(|join| !ancestors.contains(join))
        };
        let author = self.known[&id].author();
        let shaky = |other: &RecordId| {
            self.may_be_excluded(other)
                || self.is_unsettled(self.known[other].author(), self.pos[other])
        };
        let is_invite = matches!(self.actions.get(&invite), Some(Some(Action::Invite { .. })));
        let shaky = self.is_unsettled(author, at)
            || (is_invite && shaky(&invite))
            || revokes().any(|other| *other != id && shaky(other));
        if voided || !allowed {
            // Not accepted in the final pass, but maybe in an earlier one
            // (before being voided, or judged differently). Then it voided
            // the joins it hadn't seen, so none of those may be applied, and
            // whether a later join through the invite is voided too is
            // unknown: such a join re-resolves.
            if voided {
                self.excluded.insert(id);
            }
            if (voided && allowed) || shaky {
                if voids_applied {
                    return Err(Fallback::Other);
                }
                self.uncertain_invites.insert(invite);
            }
            return Ok(());
        }
        if voids_applied {
            return Err(Fallback::Voids);
        }
        self.revoked_invites.insert(invite);
        self.state.accepted.insert(id);
        self.refresh_invite(invite);
        Ok(())
    }

    /// Recomputes whether `actor` counts as 18+: their latest accepted age
    /// claim in linear order.
    fn refresh_adult(&mut self, actor: Actor) {
        let latest = self.touch.get(&actor).into_iter().flatten().rev().find_map(|id| {
            match self.actions[id] {
                Some(Action::AgeClaim { adult })
                    if self.known[id].author() == actor && self.state.accepted.contains(id) =>
                {
                    Some(adult)
                }
                _ => None,
            }
        });
        if latest == Some(true) {
            self.state.adults.insert(actor);
        } else {
            self.state.adults.remove(&actor);
        }
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
                && involved.iter().flatten().any(|actor| self.is_unsettled(*actor, i))
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
                Action::Grant { target, .. } | Action::Unban { target } => {
                    if now {
                        self.accept_write(id, &action);
                    } else {
                        self.state.accepted.remove(&id);
                        self.writes
                            .get_mut(&target)
                            .expect("accepted write")
                            .retain(|w| w.record != id);
                    }
                    dirty.insert(target);
                }
                // Removals accepted differently void different records.
                Action::Revoke { .. } | Action::Kick { .. } => return Err(Fallback::Voids),
                Action::RevokeInvite { .. } => return Err(Fallback::Voids),
                // These change which joins are accepted, and joins compete
                // for an invite's uses.
                Action::Invite { .. } | Action::Join { .. } => return Err(Fallback::Other),
                // Only the owner creates channels, and the owner's role never
                // changes, so this is never reached; re-resolve to be safe.
                Action::ChannelCreate { .. } => return Err(Fallback::Other),
                // The author's membership changed under it.
                Action::AgeClaim { .. } => {
                    if now {
                        self.state.accepted.insert(id);
                    } else {
                        self.state.accepted.remove(&id);
                    }
                    self.refresh_adult(author);
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
            match last.filter(|w| w.kicked) {
                Some(kick) => {
                    self.state.kicked.insert(actor);
                    self.state.removed.insert(actor, kick.record);
                }
                None => {
                    self.state.kicked.remove(&actor);
                    self.state.removed.remove(&actor);
                }
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

    /// What the removal `id` of `target` voids: the target's records it
    /// hadn't seen (only privileged ones for a revoke), and the joins
    /// through the target's invites it hadn't seen.
    fn concurrent_by(&self, id: RecordId, target: Actor, privileged_only: bool) -> Vec<RecordId> {
        let seen = self.ancestors(id);
        let mine = self.by_author.get(&target).into_iter().flatten();
        let own = mine.clone().copied().filter(|other| {
            !privileged_only || self.actions[other].as_ref().is_some_and(Action::is_privileged)
        });
        let invites =
            mine.filter(|other| matches!(self.actions[*other], Some(Action::Invite { .. })));
        let joins = invites.filter_map(|invite| self.invite_joins.get(invite)).flatten().copied();
        let mut voided: Vec<RecordId> =
            own.chain(joins).filter(|other| *other != id && !seen.contains(other)).collect();
        voided.sort_unstable();
        voided.dedup();
        voided
    }

    /// Whether an accepted revoke-invite, or a revoke or kick of the
    /// invite's author, voids every join through it that is new.
    fn invite_is_dead(&self, invite: RecordId) -> bool {
        if self.revoked_invites.contains(&invite) {
            return true;
        }
        let Some(Some(Action::Invite { .. })) = self.actions.get(&invite) else {
            return false;
        };
        // A removal of its author it doesn't descend from kills it.
        let author = self.known[&invite].author();
        [&self.kick_targets, &self.revoke_targets].into_iter().any(|targets| {
            targets.get(&author).is_some_and(|removals| {
                removals.iter().any(|removal| !self.is_control_ancestor(*removal, &[invite]))
            })
        })
    }

    /// Whether one of `author`'s removals in `targets` isn't an ancestor of
    /// the new record `id`, so it voids it.
    fn removed_concurrently(
        &self,
        targets: &HashMap<Actor, Vec<RecordId>>,
        author: Actor,
        id: RecordId,
    ) -> bool {
        let deps = self.known[&id].deps();
        targets.get(&author).is_some_and(|removals| {
            removals.iter().any(|removal| !self.is_control_ancestor(*removal, deps))
        })
    }

    fn ancestors(&self, id: RecordId) -> HashSet<RecordId> {
        let mut seen = HashSet::new();
        let mut stack: Vec<RecordId> = self.known[&id].deps().to_vec();
        while let Some(next) = stack.pop() {
            if seen.insert(next) {
                stack.extend_from_slice(self.known[&next].deps());
            }
        }
        seen
    }

    /// The server this resolver holds.
    pub(crate) fn server(&self) -> ServerId {
        self.server
    }

    /// The position just after the latest of `deps` in the linear order, or
    /// the deps that haven't joined the log yet.
    pub(crate) fn position_after(&self, deps: &[RecordId]) -> Result<usize, Vec<RecordId>> {
        let missing: Vec<RecordId> =
            deps.iter().filter(|dep| !self.pos.contains_key(*dep)).copied().collect();
        if !missing.is_empty() {
            return Err(missing);
        }
        Ok(deps.iter().map(|dep| self.pos[dep] + 1).max().unwrap_or(0))
    }

    /// The final pass's state just before position `at`.
    pub(crate) fn view_at(&self, at: usize) -> impl View + '_ {
        self.at(at)
    }

    /// The accepted revokes and kicks of `target`, with their record IDs.
    pub(crate) fn removals_of(&self, target: Actor) -> impl Iterator<Item = (RecordId, &Action)> {
        self.touch.get(&target).into_iter().flatten().filter_map(move |id| {
            let action = self.actions[id].as_ref()?;
            let is_removal = action.is_removal() && action.target() == Some(target);
            (is_removal && self.state.accepted.contains(id)).then_some((*id, action))
        })
    }

    /// Whether the control record `ancestor` is one of `of`'s ancestors, or
    /// `of` itself. Ancestors come earlier in the linear order, so the walk
    /// skips everything before `ancestor`.
    pub(crate) fn is_control_ancestor(&self, ancestor: RecordId, of: &[RecordId]) -> bool {
        let Some(&floor) = self.pos.get(&ancestor) else {
            return false;
        };
        let mut seen = HashSet::new();
        let mut stack: Vec<RecordId> = of.to_vec();
        while let Some(next) = stack.pop() {
            if next == ancestor {
                return true;
            }
            if self.pos.get(&next).is_some_and(|pos| *pos > floor) && seen.insert(next) {
                stack.extend_from_slice(self.known[&next].deps());
            }
        }
        false
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
        self.revoked_invites = run.revoked_invites;
        self.uncertain_invites.clear();
        self.maybe_excluded.clear();
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
    fn has_channel(&self, id: ChannelId) -> bool {
        let r = self.resolver;
        r.channel_creates
            .get(&id)
            .into_iter()
            .flatten()
            .any(|create| r.pos[create] < self.at && r.state.accepted.contains(create))
    }

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
