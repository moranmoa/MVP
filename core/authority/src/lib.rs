//! Deterministic resolution of a server's control log.
//!
//! Every peer runs [`resolve`] on the set of control records it holds. The
//! result depends only on that set, never on the order records arrived in, so
//! peers holding the same records always agree on the server state.
//! [`Resolver`] reaches the same result incrementally, one arriving record at a
//! time; `resolve` stays the reference it is tested against.
//!
//! Channel records are judged against the resolved control log by
//! [`check_channel`] (see the `channel` module).
//!
//! Rules:
//! - The genesis author is the owner and can't be demoted or kicked.
//! - Only the genesis of the record's own server counts: the server ID is
//!   derived from the owner's key (see `records::ServerId::derive`), and
//!   records of any other server are ignored.
//! - Rank: owner > admin > moderator > member. An actor can only grant a role
//!   below its own, and only revoke or kick someone ranked below it. Grants
//!   only promote; demoting is a revoke. A kicked actor can't be kicked again.
//! - Membership: the owner is a member; anyone else becomes one by a `Join`
//!   that names an invite and depends on it. Moderators and above sign
//!   invites. A join is accepted while the invite is unrevoked, has uses left
//!   (counted in linear order), hasn't expired and its author isn't kicked.
//!   A kick ends membership and blocks rejoining. Posts and grants need a
//!   member. An invite is revoked by its author or by someone ranked above
//!   them, and the revoke voids every join through it that isn't one of its
//!   causal ancestors: a join racing a revoke loses, whatever the linear
//!   order, and the joiner needs a new invite. A revoke or kick of the
//!   invite's author does the same for all their invites, so a demoted or
//!   kicked moderator's links stop working even for joins that only depend
//!   on the invite.
//! - A revoke voids the target's privileged actions that are concurrent with
//!   it (neither causal ancestors nor descendants of it). A kick voids all of
//!   the target's concurrent records. This blocks "backdating": signing new
//!   records whose deps point at state from before the removal. A record
//!   that descends from the removal was signed knowing it and is judged by
//!   the state there, so a re-granted moderator acts again, and a key the
//!   owner unbanned can join again.
//! - Only the owner unbans a kicked key. It doesn't make them a member: they
//!   join again through an invite. When two kicks of the same
//!   target are concurrent, only the first in linear order is accepted (the
//!   target is already kicked by the second), so only its voids apply.
//! - Ties between concurrent records are broken by the record ID.
//!
//! Records come from the `records` crate, so they are signed and their IDs are
//! content hashes. Records outside the control-log scope are ignored, and so
//! is a record whose body doesn't parse for its kind (it stays in the graph,
//! but is never accepted). Timestamps are informational, with one exception:
//! a join's claimed `ts` is compared with its invite's expiry. That expiry is
//! soft, an accepted risk: a joiner holding the invite can backdate `ts`.
//! Peers don't drop joins by their own clock, since peers with different
//! clocks would then disagree. The hard controls are the use limit and a
//! revoke-invite, which the client posts when an invite expires.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

pub use records::{ChannelId, PublicKey, Record, RecordId, ServerId};
use records::{Draft, Kind, RecordError, Scope, SigningKey, GENESIS_NONCE_LEN, MAX_CUT_ENTRIES};

mod channel;
mod incremental;
pub use channel::{
    check_channel, check_channel_with, message_text_ok, privileged_only, witness, ChannelGraph,
    Context, Refusal, Removal, Verdict, Void, VoidRule, MAX_MESSAGE_CODE_POINTS, MAX_WITNESS_DEPTH,
};
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
    Grant {
        target: Actor,
        role: Role,
    },
    Revoke {
        target: Actor,
        cut: Cut,
    },
    Kick {
        target: Actor,
        cut: Cut,
    },
    Post {
        text: String,
    },
    /// `expires_ms`: last join `ts` accepted, 0 for never. `max_uses`: 0 for
    /// unlimited.
    Invite {
        expires_ms: u64,
        max_uses: u32,
    },
    /// The record must also list `invite` in its deps.
    RevokeInvite {
        invite: RecordId,
    },
    /// The record must also list `invite` in its deps.
    Join {
        invite: RecordId,
    },
    /// The owner lets a kicked key back: it may join again through an invite.
    /// It isn't made a member.
    Unban {
        target: Actor,
    },
}

impl Action {
    /// A kick with an empty cut.
    pub fn kick(target: Actor) -> Action {
        Action::Kick { target, cut: Cut::default() }
    }

    /// A revoke with an empty cut.
    pub fn revoke(target: Actor) -> Action {
        Action::Revoke { target, cut: Cut::default() }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Action::Genesis => Kind::Genesis,
            Action::Grant { .. } => Kind::Grant,
            Action::Revoke { .. } => Kind::Revoke,
            Action::Kick { .. } => Kind::Kick,
            Action::Post { .. } => Kind::Post,
            Action::Invite { .. } => Kind::Invite,
            Action::RevokeInvite { .. } => Kind::RevokeInvite,
            Action::Join { .. } => Kind::Join,
            Action::Unban { .. } => Kind::Unban,
        }
    }

    /// Body encoding: target key (32 bytes), then the role byte for a grant
    /// or the cut for a revoke or kick;
    /// UTF-8 text for a post; `expires_ms` (u64 LE) then `max_uses` (u32 LE)
    /// for an invite; the invite's record ID for a revoke-invite or a join;
    /// the target key alone for an unban.
    /// A genesis body (nonce, then server metadata) is not read here: write
    /// one with [`sign_genesis`].
    pub fn body(&self) -> Vec<u8> {
        match self {
            Action::Genesis => Vec::new(),
            Action::Grant { target, role } => {
                let mut body = target.as_bytes().to_vec();
                body.push(role.code());
                body
            }
            Action::Revoke { target, cut } | Action::Kick { target, cut } => {
                let mut body = target.as_bytes().to_vec();
                cut.encode(&mut body);
                body
            }
            Action::Post { text } => text.as_bytes().to_vec(),
            Action::Invite { expires_ms, max_uses } => {
                [&expires_ms.to_le_bytes()[..], &max_uses.to_le_bytes()].concat()
            }
            Action::RevokeInvite { invite } | Action::Join { invite } => invite.as_bytes().to_vec(),
            Action::Unban { target } => target.as_bytes().to_vec(),
        }
    }

    /// Reads the action out of a control record; `None` if the body doesn't
    /// parse for the record's kind.
    pub fn from_record(record: &Record) -> Option<Action> {
        let body = record.body();
        let key = |bytes: &[u8]| Some(PublicKey::from_bytes(bytes.try_into().ok()?));
        let id = |bytes: &[u8]| Some(RecordId::from_bytes(bytes.try_into().ok()?));
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
            Kind::Revoke | Kind::Kick => {
                let (target, cut) = body.split_at_checked(32)?;
                let (target, cut) = (key(target)?, Cut::parse(cut)?);
                Some(if record.kind() == Kind::Kick {
                    Action::Kick { target, cut }
                } else {
                    Action::Revoke { target, cut }
                })
            }
            Kind::Post => Some(Action::Post { text: String::from_utf8(body.to_vec()).ok()? }),
            Kind::Invite => {
                let (expires, uses) = body.split_at_checked(8)?;
                Some(Action::Invite {
                    expires_ms: u64::from_le_bytes(expires.try_into().ok()?),
                    max_uses: u32::from_le_bytes(uses.try_into().ok()?),
                })
            }
            Kind::RevokeInvite => Some(Action::RevokeInvite { invite: id(body)? }),
            Kind::Join => Some(Action::Join { invite: id(body)? }),
            Kind::Unban => Some(Action::Unban { target: key(body)? }),
            // Channel kinds are judged by the channel checks (B4-4); server,
            // channel and leave records come in B4-3. Until then they are
            // never accepted.
            Kind::ServerEdit
            | Kind::ServerClose
            | Kind::ChannelCreate
            | Kind::ChannelEdit
            | Kind::ChannelDelete
            | Kind::Leave
            | Kind::AgeClaim
            | Kind::Message
            | Kind::MessageEdit
            | Kind::MessageDelete
            | Kind::KeyRotation => None,
        }
    }

    /// The actor whose role is changed by a grant, revoke, kick or unban.
    fn target(&self) -> Option<Actor> {
        match self {
            Action::Grant { target, .. }
            | Action::Revoke { target, .. }
            | Action::Kick { target, .. }
            | Action::Unban { target } => Some(*target),
            Action::Genesis
            | Action::Post { .. }
            | Action::Invite { .. }
            | Action::RevokeInvite { .. }
            | Action::Join { .. } => None,
        }
    }

    /// The actor whose role history this action writes to, if any: the
    /// target, or the author of a join.
    fn subject(&self, author: Actor) -> Option<Actor> {
        match self {
            Action::Join { .. } => Some(author),
            _ => self.target(),
        }
    }

    /// The invite a join or revoke-invite names.
    fn invite(&self) -> Option<RecordId> {
        match self {
            Action::RevokeInvite { invite } | Action::Join { invite } => Some(*invite),
            _ => None,
        }
    }

    /// Needs a rank above member; a revoke voids these when concurrent.
    fn is_privileged(&self) -> bool {
        self.target().is_some()
            || matches!(self, Action::Invite { .. } | Action::RevokeInvite { .. })
    }

    fn is_removal(&self) -> bool {
        matches!(self, Action::Revoke { .. } | Action::Kick { .. })
    }
}

/// The channel heads a revoke, kick or leave was signed on. A channel
/// usually has several concurrent heads, and the remover has seen them all,
/// so a cut may name several heads per channel. Once the removal is
/// accepted, a channel record of the target survives if it is an ancestor
/// of any head of its channel in the cut; the rest of the target's records
/// in that channel are voided (GAP-2, B4-4). A channel the cut doesn't name
/// is not voided at all.
///
/// Encoding (see `records::CUT_BYTES`): `count u8`, then `count` pairs of
/// (channel ID, head record ID), strictly ascending by (channel, head), at
/// most `records::MAX_CUT_ENTRIES` pairs in total. Anything else doesn't
/// parse, so each cut has one encoding.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cut(Vec<(ChannelId, RecordId)>);

impl Cut {
    /// A cut naming `heads`, sorted and deduplicated; `None` if more than
    /// `records::MAX_CUT_ENTRIES` distinct pairs remain.
    pub fn new(mut heads: Vec<(ChannelId, RecordId)>) -> Option<Cut> {
        heads.sort();
        heads.dedup();
        (heads.len() <= MAX_CUT_ENTRIES).then_some(Cut(heads))
    }

    /// Every (channel, head) pair, in ascending order.
    pub fn heads(&self) -> &[(ChannelId, RecordId)] {
        &self.0
    }

    /// The heads the cut names for `channel`; empty if it names none.
    pub fn heads_of(&self, channel: ChannelId) -> impl Iterator<Item = RecordId> + '_ {
        let start = self.0.partition_point(|(c, _)| *c < channel);
        self.0[start..].iter().take_while(move |(c, _)| *c == channel).map(|(_, head)| *head)
    }

    /// Whether the cut names `channel` at all.
    pub fn names(&self, channel: ChannelId) -> bool {
        self.heads_of(channel).next().is_some()
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::try_from(self.0.len()).expect("at most MAX_CUT_ENTRIES"));
        for (channel, head) in &self.0 {
            out.extend_from_slice(channel.as_bytes());
            out.extend_from_slice(head.as_bytes());
        }
    }

    fn parse(bytes: &[u8]) -> Option<Cut> {
        let (&count, pairs) = bytes.split_first()?;
        let count = usize::from(count);
        if count > MAX_CUT_ENTRIES || pairs.len() != count * 64 {
            return None;
        }
        let heads: Vec<(ChannelId, RecordId)> = pairs
            .chunks_exact(64)
            .map(|pair| {
                let (channel, head) = pair.split_at(32);
                let channel = ChannelId::from_bytes(channel.try_into().expect("32 bytes"));
                (channel, RecordId::from_bytes(head.try_into().expect("32 bytes")))
            })
            .collect();
        heads.windows(2).all(|pair| pair[0] < pair[1]).then_some(Cut(heads))
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
    /// The kicked actors and the kick that removed each one, for the owner's
    /// list of removed accounts (`listRemovedMembers`); the kick record's `ts`
    /// is when it happened.
    pub removed: BTreeMap<Actor, RecordId>,
    /// Current members, the owner included. This is the member set
    /// `groupkeys` rotates to.
    pub members: BTreeSet<Actor>,
    /// Accepted invites by record ID.
    pub invites: BTreeMap<RecordId, Invite>,
    /// Accepted posts, in display order.
    pub posts: Vec<RecordId>,
    pub accepted: BTreeSet<RecordId>,
}

/// An accepted invite and how it has been used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invite {
    pub author: Actor,
    /// 0 for never.
    pub expires_ms: u64,
    /// 0 for unlimited.
    pub max_uses: u32,
    /// Accepted joins through it.
    pub uses: u32,
    pub revoked: bool,
}

impl State {
    pub fn role(&self, actor: Actor) -> Role {
        self.role_of(actor)
    }

    pub fn is_member(&self, actor: Actor) -> bool {
        self.members.contains(&actor)
    }
}

/// The parts of the state that decide whether a record is permitted.
trait View {
    fn owner(&self) -> Option<Actor>;
    fn assigned_role(&self, actor: Actor) -> Role;
    fn is_kicked(&self, actor: Actor) -> bool;
    /// Joined and not removed since; the owner is a member regardless.
    fn has_joined(&self, actor: Actor) -> bool;
    /// An accepted invite as it stands at this point.
    fn invite(&self, id: RecordId) -> Option<Invite>;

    fn role_of(&self, actor: Actor) -> Role {
        if self.owner() == Some(actor) {
            Role::Owner
        } else {
            self.assigned_role(actor)
        }
    }

    fn is_member(&self, actor: Actor) -> bool {
        self.owner() == Some(actor) || self.has_joined(actor)
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

    fn has_joined(&self, actor: Actor) -> bool {
        self.members.contains(&actor)
    }

    fn invite(&self, id: RecordId) -> Option<Invite> {
        self.invites.get(&id).copied()
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
            author_role > *role
                && *role > view.role_of(*target)
                && !view.is_kicked(*target)
                && view.is_member(*target)
        }
        Action::Revoke { target, .. } => {
            let target_role = view.role_of(*target);
            author_role > target_role && target_role > Role::Member
        }
        Action::Kick { target, .. } => {
            author_role >= Role::Moderator
                && author_role > view.role_of(*target)
                && !view.is_kicked(*target)
        }
        Action::Post { .. } => view.is_member(author),
        Action::Invite { .. } => author_role >= Role::Moderator,
        Action::RevokeInvite { invite } => {
            record.deps().contains(invite)
                && view.invite(*invite).is_some_and(|invite| {
                    !invite.revoked
                        && (invite.author == author || author_role > view.role_of(invite.author))
                })
        }
        Action::Join { invite } => {
            record.deps().contains(invite)
                && !view.is_member(author)
                && view.invite(*invite).is_some_and(|invite| {
                    !invite.revoked
                        && !view.is_kicked(invite.author)
                        && (invite.max_uses == 0 || invite.uses < invite.max_uses)
                        && (invite.expires_ms == 0 || record.ts() <= invite.expires_ms)
                })
        }
        // Undoing a kick is the owner's alone (spec B11, ש-4).
        Action::Unban { target } => author_role == Role::Owner && view.is_kicked(*target),
    }
}

/// What an accepted record left its subject with. Every write is a full
/// snapshot: a role above member implies membership (grants need a member,
/// and kicks reset the role), so no write depends on the state before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Write {
    record: RecordId,
    role: Role,
    kicked: bool,
    member: bool,
}

/// Applies an accepted genesis, privileged record or join to the roles and
/// members. Posts are left to the caller, which knows where they go in
/// display order, and so are invite uses.
fn apply(state: &mut State, record: &Record, action: &Action) -> Option<(Actor, Write)> {
    let write = |role, kicked, member| Write { record: record.id(), role, kicked, member };
    match action {
        Action::Genesis => {
            state.owner = Some(record.author());
            state.members.insert(record.author());
            None
        }
        Action::Grant { target, role } => {
            // Never Member: grants only promote.
            state.roles.insert(*target, *role);
            Some((*target, write(*role, false, true)))
        }
        Action::Revoke { target, .. } => {
            // Only roles above member are revoked, so the target is a member.
            state.roles.remove(target);
            Some((*target, write(Role::Member, false, true)))
        }
        Action::Kick { target, .. } => {
            state.roles.remove(target);
            state.kicked.insert(*target);
            state.members.remove(target);
            state.removed.insert(*target, record.id());
            Some((*target, write(Role::Member, true, false)))
        }
        Action::Join { .. } => {
            state.members.insert(record.author());
            Some((record.author(), write(Role::Member, false, true)))
        }
        Action::Unban { target } => {
            // Not a member again: they rejoin through an invite.
            state.kicked.remove(target);
            state.removed.remove(target);
            Some((*target, write(Role::Member, false, false)))
        }
        Action::Post { .. } | Action::Invite { .. } | Action::RevokeInvite { .. } => None,
    }
}

/// Updates the invites for an accepted invite, revoke-invite or join, as
/// the linear-order fold reaches it.
fn apply_invite(state: &mut State, record: &Record, action: &Action) {
    match action {
        Action::Invite { expires_ms, max_uses } => {
            state.invites.insert(
                record.id(),
                Invite {
                    author: record.author(),
                    expires_ms: *expires_ms,
                    max_uses: *max_uses,
                    uses: 0,
                    revoked: false,
                },
            );
        }
        Action::RevokeInvite { invite } => {
            if let Some(invite) = state.invites.get_mut(invite) {
                invite.revoked = true;
            }
        }
        Action::Join { invite } => {
            if let Some(invite) = state.invites.get_mut(invite) {
                invite.uses = invite.uses.saturating_add(1);
            }
        }
        _ => {}
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
    /// Kicks and revokes accepted in any pass, by target.
    kick_targets: HashMap<Actor, Vec<RecordId>>,
    revoke_targets: HashMap<Actor, Vec<RecordId>>,
    /// Invites with a revoke-invite accepted in any pass.
    revoked_invites: HashSet<RecordId>,
    /// Actors whose role history differs between passes: some pass accepted
    /// a privileged record about them that the final pass didn't, or the
    /// other way round. Only the final pass's history is kept. When every
    /// pass accepted the same first kick of the actor and their history only
    /// differs before it, it is the same in all passes from that kick on: the
    /// kick is kept as the bound.
    unsettled: HashMap<Actor, Option<RecordId>>,
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
    let mut kick_targets: HashMap<Actor, Vec<RecordId>> = HashMap::new();
    let mut revoke_targets: HashMap<Actor, Vec<RecordId>> = HashMap::new();
    let mut revoked_invites = HashSet::new();
    let mut earlier_passes: Vec<(BTreeSet<usize>, HashMap<Actor, usize>)> = Vec::new();
    loop {
        let pass = graph.fold(&excluded);
        for (targets, removals) in
            [(&mut kick_targets, pass.kick_targets), (&mut revoke_targets, pass.revoke_targets)]
        {
            for (target, removal) in removals {
                let ids = targets.entry(target).or_default();
                if !ids.contains(&removal) {
                    ids.push(removal);
                }
            }
        }
        revoked_invites.extend(pass.revoked_invites);
        let fresh: Vec<usize> = pass.voided.into_iter().filter(|i| !excluded[*i]).collect();
        if fresh.is_empty() {
            // Actor -> the last position where their history differs.
            let mut differs: HashMap<Actor, usize> = HashMap::new();
            let mut owner_unsettled = false;
            for (settled, _) in &earlier_passes {
                for &i in pass.settled.symmetric_difference(settled) {
                    let author = graph.records[i].author();
                    match graph.actions[i].as_ref().and_then(|a| a.subject(author)) {
                        Some(target) => {
                            let last = differs.entry(target).or_insert(i);
                            *last = (*last).max(i);
                        }
                        None => owner_unsettled = true,
                    }
                }
            }
            // Writes are full snapshots, so when every pass accepted the same
            // first kick of an actor and their history differs only before
            // it, it is the same in all passes from that kick on.
            let unsettled = differs
                .into_iter()
                .map(|(actor, last)| {
                    let kick = pass.first_kicks.get(&actor).copied().filter(|&kick| {
                        last < kick
                            && earlier_passes
                                .iter()
                                .all(|(_, kicks)| kicks.get(&actor) == Some(&kick))
                    });
                    (actor, kick.map(|kick| order[kick]))
                })
                .collect();
            let excluded = (0..order.len()).filter(|i| excluded[*i]).map(|i| order[i]).collect();
            return Run {
                order,
                state: pass.state,
                excluded,
                kick_targets,
                revoke_targets,
                revoked_invites,
                unsettled,
                owner_unsettled,
                writes: pass.writes,
                genesis: pass.genesis,
            };
        }
        earlier_passes.push((pass.settled, pass.first_kicks));
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
    /// Positions of the records that list each record in their deps.
    children: Vec<Vec<usize>>,
    by_author: HashMap<Actor, Vec<usize>>,
    /// Invite -> positions of the joins naming it.
    joins: HashMap<RecordId, Vec<usize>>,
    /// What each accepted removal voids, worked out the first time a pass
    /// accepts it. Only removals need to know their ancestors, so a log full
    /// of posts costs no ancestry memory.
    voids: RefCell<HashMap<usize, Vec<usize>>>,
}

struct Pass {
    state: State,
    voided: Vec<usize>,
    /// Positions of accepted records that write a role history: the
    /// genesis, grants, revokes, kicks, joins and unbans.
    settled: BTreeSet<usize>,
    /// Accepted kicks and revokes: (target, record).
    kick_targets: Vec<(Actor, RecordId)>,
    revoke_targets: Vec<(Actor, RecordId)>,
    revoked_invites: Vec<RecordId>,
    /// Position of the first accepted kick of each kicked actor.
    first_kicks: HashMap<Actor, usize>,
    writes: HashMap<Actor, Vec<Write>>,
    genesis: Option<RecordId>,
}

impl<'a> Graph<'a> {
    fn new(by_id: &HashMap<RecordId, &'a Record>, order: &[RecordId]) -> Self {
        let position: HashMap<RecordId, usize> =
            order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let records: Vec<&Record> = order.iter().map(|id| by_id[id]).collect();
        let deps: Vec<Vec<usize>> =
            records.iter().map(|r| r.deps().iter().map(|dep| position[dep]).collect()).collect();
        let mut by_author: HashMap<Actor, Vec<usize>> = HashMap::new();
        for (i, record) in records.iter().enumerate() {
            by_author.entry(record.author()).or_default().push(i);
        }
        let actions: Vec<Option<Action>> = records.iter().map(|r| Action::from_record(r)).collect();
        let mut joins: HashMap<RecordId, Vec<usize>> = HashMap::new();
        for (i, action) in actions.iter().enumerate() {
            // Only a join that depends on its invite can be accepted.
            if let Some(Action::Join { invite }) = action {
                if records[i].deps().contains(invite) {
                    joins.entry(*invite).or_default().push(i);
                }
            }
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); records.len()];
        for (i, deps) in deps.iter().enumerate() {
            for &dep in deps {
                children[dep].push(i);
            }
        }
        Graph { records, actions, deps, children, by_author, joins, voids: RefCell::default() }
    }

    fn fold(&self, excluded: &[bool]) -> Pass {
        let mut pass = Pass {
            state: State::default(),
            voided: Vec::new(),
            settled: BTreeSet::new(),
            kick_targets: Vec::new(),
            revoke_targets: Vec::new(),
            revoked_invites: Vec::new(),
            first_kicks: HashMap::new(),
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
            apply_invite(&mut pass.state, record, action);
            match action {
                Action::Post { .. } => {
                    pass.state.posts.push(record.id());
                    continue;
                }
                Action::Invite { .. } => continue,
                Action::RevokeInvite { invite } => {
                    pass.voided.extend(self.voids_of(i));
                    pass.revoked_invites.push(*invite);
                    continue;
                }
                Action::Genesis => pass.genesis = Some(record.id()),
                Action::Grant { .. } | Action::Join { .. } | Action::Unban { .. } => {}
                Action::Revoke { target, .. } => {
                    pass.voided.extend(self.voids_of(i));
                    pass.revoke_targets.push((*target, record.id()));
                }
                Action::Kick { target, .. } => {
                    pass.voided.extend(self.voids_of(i));
                    pass.kick_targets.push((*target, record.id()));
                    pass.first_kicks.entry(*target).or_insert(i);
                }
            }
            if let Some((target, write)) = apply(&mut pass.state, record, action) {
                pass.writes.entry(target).or_default().push(write);
            }
            pass.settled.insert(i);
        }
        pass
    }

    /// What the accepted removal or revoke-invite at `removal` voids, of the
    /// records it hadn't seen: for a kick, all the target's records; for a
    /// revoke, the target's privileged ones; for both, the joins through
    /// the target's invites (a removed moderator's links stop working); for
    /// a revoke-invite, the joins through that invite. Cached per record;
    /// what it voids is read from the record itself, so the cache key says
    /// everything.
    fn voids_of(&self, removal: usize) -> Vec<usize> {
        let (own, privileged_only, invites) = match &self.actions[removal] {
            Some(Action::Revoke { target, .. }) => {
                (self.by_author.get(target), true, self.invites_by(*target))
            }
            Some(Action::Kick { target, .. }) => {
                (self.by_author.get(target), false, self.invites_by(*target))
            }
            Some(Action::RevokeInvite { invite }) => (None, false, vec![*invite]),
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
            // Only the target's records concurrent with the removal: one that
            // descends from it was signed knowing it and is judged by state.
            // Descendants come later in the linear order.
            let mut below = vec![false; self.records.len()];
            let mut stack = self.children[removal].clone();
            while let Some(i) = stack.pop() {
                if !std::mem::replace(&mut below[i], true) {
                    stack.extend_from_slice(&self.children[i]);
                }
            }
            let own = own.into_iter().flatten().copied().filter(|&i| {
                (!privileged_only || self.actions[i].as_ref().is_some_and(Action::is_privileged))
                    && !below[i]
            });
            let joins =
                invites.iter().filter_map(|invite| self.joins.get(invite)).flatten().copied();
            let mut voided: Vec<usize> = own
                .chain(joins)
                .filter(|&i| i != removal && !seen.get(i).copied().unwrap_or(false))
                .collect();
            voided.sort_unstable();
            voided.dedup();
            voided
        });
        voided.clone()
    }

    /// The invites `author` signed.
    fn invites_by(&self, author: Actor) -> Vec<RecordId> {
        let mine = self.by_author.get(&author).into_iter().flatten();
        mine.filter(|&&i| matches!(self.actions[i], Some(Action::Invite { .. })))
            .map(|&i| self.records[i].id())
            .collect()
    }
}
