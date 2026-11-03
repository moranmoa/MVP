//! Deterministic resolution of a server's control log.
//!
//! Every peer runs [`resolve`] on the set of control records it holds. The
//! result depends only on that set, never on the order records arrived in, so
//! peers holding the same records always agree on the server state.
//! [`Resolver`] reaches the same result incrementally, one arriving record at a
//! time; `resolve` stays the reference it is tested against.
//!
//! Rules:
//! - The genesis author is the owner and can't be demoted or kicked.
//! - Only the genesis of the record's own server counts: the server ID is
//!   derived from the owner's key (see `records::ServerId::derive`), and
//!   records of any other server are ignored.
//! - Rank: owner > admin > moderator > member. An actor can only grant a role
//!   below its own, and only revoke or kick someone ranked below it. Grants
//!   only promote; demoting is a revoke. A kicked actor can't be kicked again.
//! - A revoke voids the target's privileged actions that are concurrent with
//!   it (not causal ancestors of it). A kick voids all of the target's
//!   concurrent records. This blocks "backdating": signing new records whose
//!   deps point at state from before the removal. When two kicks of the same
//!   target are concurrent, only the first in linear order is accepted (the
//!   target is already kicked by the second), so only its voids apply.
//! - Ties between concurrent records are broken by the record ID.
//!
//! Records come from the `records` crate, so they are signed and their IDs are
//! content hashes. Records outside the control-log scope are ignored, and so
//! is a record whose body doesn't parse for its kind (it stays in the graph,
//! but is never accepted). Timestamps are informational and never consulted.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use records::{Draft, Kind, RecordError, Scope, SigningKey, GENESIS_NONCE_LEN};
pub use records::{PublicKey, Record, RecordId, ServerId};

mod incremental;
pub use incremental::{Inserted, Resolver, Stats, MAX_PENDING};

/// An actor is identified by their Ed25519 public key.
pub type Actor = PublicKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Member,
    Moderator,
    Admin,
    Owner,
}

impl Role {
    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(code: u8) -> Option<Role> {
        [Role::Member, Role::Moderator, Role::Admin, Role::Owner].get(usize::from(code)).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Genesis,
    Grant { target: Actor, role: Role },
    Revoke { target: Actor },
    Kick { target: Actor },
    Post { text: String },
}

impl Action {
    pub fn kind(&self) -> Kind {
        match self {
            Action::Genesis => Kind::Genesis,
            Action::Grant { .. } => Kind::Grant,
            Action::Revoke { .. } => Kind::Revoke,
            Action::Kick { .. } => Kind::Kick,
            Action::Post { .. } => Kind::Post,
        }
    }

    /// Body encoding: target key (32 bytes), then the role byte for a grant;
    /// UTF-8 text for a post. A genesis body (nonce, then server metadata) is
    /// not read here: write one with [`sign_genesis`].
    pub fn body(&self) -> Vec<u8> {
        match self {
            Action::Genesis => Vec::new(),
            Action::Grant { target, role } => {
                let mut body = target.as_bytes().to_vec();
                body.push(role.code());
                body
            }
            Action::Revoke { target } | Action::Kick { target } => target.as_bytes().to_vec(),
            Action::Post { text } => text.as_bytes().to_vec(),
        }
    }

    /// Reads the action out of a control record; `None` if the body doesn't
    /// parse for the record's kind.
    pub fn from_record(record: &Record) -> Option<Action> {
        let body = record.body();
        let key = |bytes: &[u8]| Some(PublicKey::from_bytes(bytes.try_into().ok()?));
        match record.kind() {
            Kind::Genesis => Some(Action::Genesis),
            Kind::Grant => {
                let (target, role) = body.split_at_checked(32)?;
                let [role] = role else { return None };
                // Only moderator and admin can be granted: demoting is a
                // revoke, and ownership comes from the genesis.
                let role =
                    Role::from_code(*role).filter(|r| *r > Role::Member && *r < Role::Owner)?;
                Some(Action::Grant { target: key(target)?, role })
            }
            Kind::Revoke => Some(Action::Revoke { target: key(body)? }),
            Kind::Kick => Some(Action::Kick { target: key(body)? }),
            Kind::Post => Some(Action::Post { text: String::from_utf8(body.to_vec()).ok()? }),
            Kind::Message => None,
        }
    }

    fn target(&self) -> Option<Actor> {
        match self {
            Action::Grant { target, .. } | Action::Revoke { target } | Action::Kick { target } => {
                Some(*target)
            }
            Action::Genesis | Action::Post { .. } => None,
        }
    }

    fn is_privileged(&self) -> bool {
        self.target().is_some()
    }

    fn is_removal(&self) -> bool {
        matches!(self, Action::Revoke { .. } | Action::Kick { .. })
    }
}

/// Signs a control-log record carrying `action`.
pub fn sign_control(
    key: &SigningKey,
    server: ServerId,
    seq: u64,
    deps: Vec<RecordId>,
    ts: u64,
    action: &Action,
) -> Result<Record, RecordError> {
    Draft {
        server,
        scope: Scope::Control,
        seq,
        deps,
        auth_deps: Vec::new(),
        ts,
        kind: action.kind(),
        body: action.body(),
    }
    .sign(key)
}

/// Creates a new server: signs its genesis with the owner's key. The server
/// ID is derived from that key and `nonce` (pick a fresh random one per
/// server), so nobody else can write a genesis for it.
pub fn sign_genesis(
    key: &SigningKey,
    nonce: [u8; GENESIS_NONCE_LEN],
    ts: u64,
    metadata: &[u8],
) -> Result<Record, RecordError> {
    let owner = PublicKey::from_bytes(key.verifying_key().to_bytes());
    Draft {
        server: ServerId::derive(&owner, &nonce),
        scope: Scope::Control,
        seq: 0,
        deps: Vec::new(),
        auth_deps: Vec::new(),
        ts,
        kind: Kind::Genesis,
        body: [&nonce[..], metadata].concat(),
    }
    .sign(key)
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
        self.role_of(actor)
    }
}

/// The parts of the state that decide whether a record is permitted.
trait View {
    fn owner(&self) -> Option<Actor>;
    fn assigned_role(&self, actor: Actor) -> Role;
    fn is_kicked(&self, actor: Actor) -> bool;

    fn role_of(&self, actor: Actor) -> Role {
        if self.owner() == Some(actor) {
            Role::Owner
        } else {
            self.assigned_role(actor)
        }
    }
}

impl View for State {
    fn owner(&self) -> Option<Actor> {
        self.owner
    }

    fn assigned_role(&self, actor: Actor) -> Role {
        self.roles.get(&actor).copied().unwrap_or(Role::Member)
    }

    fn is_kicked(&self, actor: Actor) -> bool {
        self.kicked.contains(&actor)
    }
}

fn permitted(view: &impl View, record: &Record, action: &Action) -> bool {
    let author = record.author();
    if view.is_kicked(author) {
        return false;
    }
    if *action == Action::Genesis {
        return view.owner().is_none() && record.deps().is_empty();
    }
    if view.owner().is_none() {
        return false;
    }
    let author_role = view.role_of(author);
    match action {
        Action::Genesis => unreachable!("handled above"),
        // Grants only promote. A demotion has to be a revoke (then a new
        // grant if wanted), so it voids the target's concurrent privileged
        // records like any revoke: a demoted moderator can't backdate a kick.
        Action::Grant { target, role } => {
            author_role > *role && *role > view.role_of(*target) && !view.is_kicked(*target)
        }
        Action::Revoke { target } => {
            let target_role = view.role_of(*target);
            author_role > target_role && target_role > Role::Member
        }
        Action::Kick { target } => {
            author_role >= Role::Moderator
                && author_role > view.role_of(*target)
                && !view.is_kicked(*target)
        }
        Action::Post { .. } => true,
    }
}

/// What an accepted privileged record left its target with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Write {
    record: RecordId,
    role: Role,
    kicked: bool,
}

/// Applies an accepted genesis or privileged record to the roles. Posts are
/// left to the caller, which knows where they go in display order.
fn apply(state: &mut State, record: &Record, action: &Action) -> Option<(Actor, Write)> {
    let write = |role, kicked| Write { record: record.id(), role, kicked };
    match action {
        Action::Genesis => {
            state.owner = Some(record.author());
            None
        }
        Action::Grant { target, role } => {
            // Never Member: grants only promote.
            state.roles.insert(*target, *role);
            Some((*target, write(*role, false)))
        }
        Action::Revoke { target } => {
            state.roles.remove(target);
            Some((*target, write(Role::Member, false)))
        }
        Action::Kick { target } => {
            state.roles.remove(target);
            state.kicked.insert(*target);
            Some((*target, write(Role::Member, true)))
        }
        Action::Post { .. } => None,
    }
}

/// Computes the server state from a set of control records. Records whose
/// deps aren't all in the set, and records of any other server, are ignored.
pub fn resolve(server: ServerId, records: &[Record]) -> State {
    full_run(server, records).state
}

/// Everything the reference resolution learns on the way to the state. The
/// incremental resolver keeps these up to date so it can tell when a new
/// record would change the outcome of earlier passes.
struct Run {
    order: Vec<RecordId>,
    state: State,
    /// Records voided in any pass.
    excluded: HashSet<RecordId>,
    /// Targets of kicks and revokes accepted in any pass.
    kick_targets: HashSet<Actor>,
    revoke_targets: HashSet<Actor>,
    /// Actors whose role history differs between passes: some pass accepted
    /// a privileged record about them that the final pass didn't, or the
    /// other way round. Only the final pass's history is kept.
    unsettled: HashSet<Actor>,
    /// Same, for the genesis: ownership differed between passes.
    owner_unsettled: bool,
    /// Final pass: the writes each actor received, in linear order.
    writes: HashMap<Actor, Vec<Write>>,
    genesis: Option<RecordId>,
}

fn full_run<'a>(server: ServerId, records: impl IntoIterator<Item = &'a Record>) -> Run {
    let by_id: HashMap<RecordId, &Record> = records
        .into_iter()
        .filter(|r| r.server() == server && r.scope() == Scope::Control)
        .map(|r| (r.id(), r))
        .collect();
    let order = linear_order(&by_id);
    let graph = Graph::new(&by_id, &order);

    // A valid removal can void records that came earlier in the linear
    // order, so fold again until no new records get voided. The excluded set
    // only grows, so this terminates.
    let mut excluded = vec![false; order.len()];
    let mut kick_targets = HashSet::new();
    let mut revoke_targets = HashSet::new();
    let mut earlier_passes: Vec<BTreeSet<usize>> = Vec::new();
    loop {
        let pass = graph.fold(&excluded);
        kick_targets.extend(pass.kick_targets);
        revoke_targets.extend(pass.revoke_targets);
        let fresh: Vec<usize> = pass.voided.into_iter().filter(|i| !excluded[*i]).collect();
        if fresh.is_empty() {
            let mut unsettled = HashSet::new();
            let mut owner_unsettled = false;
            for settled in &earlier_passes {
                for &i in pass.settled.symmetric_difference(settled) {
                    match graph.actions[i].as_ref().and_then(Action::target) {
                        Some(target) => {
                            unsettled.insert(target);
                        }
                        None => owner_unsettled = true,
                    }
                }
            }
            let excluded = (0..order.len()).filter(|i| excluded[*i]).map(|i| order[i]).collect();
            return Run {
                order,
                state: pass.state,
                excluded,
                kick_targets,
                revoke_targets,
                unsettled,
                owner_unsettled,
                writes: pass.writes,
                genesis: pass.genesis,
            };
        }
        earlier_passes.push(pass.settled);
        for i in fresh {
            excluded[i] = true;
        }
    }
}

/// Topological order of the record graph; concurrent records go in ID order.
/// A record whose dependency chain isn't all in `by_id` never becomes ready,
/// so it is left out, in one linear pass however long the broken chain is.
fn linear_order(by_id: &HashMap<RecordId, &Record>) -> Vec<RecordId> {
    let mut pending_deps: HashMap<RecordId, usize> =
        by_id.values().map(|r| (r.id(), r.deps().len())).collect();
    let mut dependents: HashMap<RecordId, Vec<RecordId>> = HashMap::new();
    for record in by_id.values() {
        for dep in record.deps() {
            dependents.entry(*dep).or_default().push(record.id());
        }
    }

    let mut ready: BinaryHeap<Reverse<RecordId>> =
        pending_deps.iter().filter(|(_, count)| **count == 0).map(|(id, _)| Reverse(*id)).collect();
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

/// The complete records laid out in linear order, indexed by position.
struct Graph<'a> {
    records: Vec<&'a Record>,
    actions: Vec<Option<Action>>,
    /// Positions of each record's deps.
    deps: Vec<Vec<usize>>,
    by_author: HashMap<Actor, Vec<usize>>,
    /// What each accepted removal voids, worked out the first time a pass
    /// accepts it. Only removals need to know their ancestors, so a log full
    /// of posts costs no ancestry memory.
    voids: RefCell<HashMap<usize, Vec<usize>>>,
}

struct Pass {
    state: State,
    voided: Vec<usize>,
    /// Positions of accepted genesis and privileged records.
    settled: BTreeSet<usize>,
    kick_targets: Vec<Actor>,
    revoke_targets: Vec<Actor>,
    writes: HashMap<Actor, Vec<Write>>,
    genesis: Option<RecordId>,
}

impl<'a> Graph<'a> {
    fn new(by_id: &HashMap<RecordId, &'a Record>, order: &[RecordId]) -> Self {
        let position: HashMap<RecordId, usize> =
            order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let records: Vec<&Record> = order.iter().map(|id| by_id[id]).collect();
        let deps =
            records.iter().map(|r| r.deps().iter().map(|dep| position[dep]).collect()).collect();
        let mut by_author: HashMap<Actor, Vec<usize>> = HashMap::new();
        for (i, record) in records.iter().enumerate() {
            by_author.entry(record.author()).or_default().push(i);
        }
        let actions = records.iter().map(|r| Action::from_record(r)).collect();
        Graph { records, actions, deps, by_author, voids: RefCell::default() }
    }

    fn fold(&self, excluded: &[bool]) -> Pass {
        let mut pass = Pass {
            state: State::default(),
            voided: Vec::new(),
            settled: BTreeSet::new(),
            kick_targets: Vec::new(),
            revoke_targets: Vec::new(),
            writes: HashMap::new(),
            genesis: None,
        };
        for (i, record) in self.records.iter().enumerate() {
            if excluded[i] {
                continue;
            }
            let Some(action) = &self.actions[i] else {
                continue;
            };
            if !permitted(&pass.state, record, action) {
                continue;
            }
            pass.state.accepted.insert(record.id());
            match action {
                Action::Post { .. } => {
                    pass.state.posts.push(record.id());
                    continue;
                }
                Action::Genesis => pass.genesis = Some(record.id()),
                Action::Grant { .. } => {}
                Action::Revoke { target } => {
                    pass.voided.extend(self.voids_of(i));
                    pass.revoke_targets.push(*target);
                }
                Action::Kick { target } => {
                    pass.voided.extend(self.voids_of(i));
                    pass.kick_targets.push(*target);
                }
            }
            if let Some((target, write)) = apply(&mut pass.state, record, action) {
                pass.writes.entry(target).or_default().push(write);
            }
            pass.settled.insert(i);
        }
        pass
    }

    /// What the accepted removal at `removal` voids: the target's records it
    /// hadn't seen (only privileged ones for a revoke). Cached per removal;
    /// the target and kind are read from the record itself, so the cache key
    /// says everything.
    fn voids_of(&self, removal: usize) -> Vec<usize> {
        let (target, privileged_only) = match &self.actions[removal] {
            Some(Action::Revoke { target }) => (*target, true),
            Some(Action::Kick { target }) => (*target, false),
            _ => return Vec::new(),
        };
        let mut voids = self.voids.borrow_mut();
        let voided = voids.entry(removal).or_insert_with(|| {
            // Ancestors come earlier in the linear order.
            let mut seen = vec![false; removal];
            let mut stack = self.deps[removal].clone();
            while let Some(i) = stack.pop() {
                if !std::mem::replace(&mut seen[i], true) {
                    stack.extend_from_slice(&self.deps[i]);
                }
            }
            let by_target = self.by_author.get(&target).into_iter().flatten().copied();
            by_target
                .filter(|&i| {
                    i != removal
                        && !seen.get(i).copied().unwrap_or(false)
                        && (!privileged_only
                            || self.actions[i].as_ref().is_some_and(Action::is_privileged))
                })
                .collect()
        });
        voided.clone()
    }
}
