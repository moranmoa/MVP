//! Channel records judged against the control log (GAP-2, GAP-5).
//!
//! A channel record names the control-log heads it relies on in its
//! `auth_deps`. [`check_channel`] judges it against the resolved control
//! state just after the latest of those heads:
//!
//! - Every `auth_dep` must have joined the control log; otherwise the verdict
//!   is [`Verdict::Pending`], never accept.
//! - The author must be a member there (not kicked).
//! - `Message`: any member. `MessageEdit`: the original author only.
//!   `MessageDelete`: the original author, or a moderator or above who
//!   outranks them. `KeyRotation`: [`may_rotate`], moderator or above.
//!   Edits and deletes name a `Message`, which must be in their deps.
//! - A record that passes can still be voided by an accepted kick or revoke
//!   of its author that it is concurrent with, that is, none of its
//!   `auth_deps` has the removal as an ancestor. Which records that voids is
//!   the [`VoidRule`], swappable until the owner decides between
//!   [`witness`] (A, the default) and [`privileged_only`] (B).
//!
//! Under A a record can be saved by a witness: an accepted record of another
//! author that descends from it. Whether the witness is accepted only
//! depends on records after it, so verdicts are computed descendants first
//! and memoized within a check, without iterating, and the result depends
//! only on the records held. The walk costs at most the target's
//! unwitnessed tail of the channel.
//!
//! The caller re-checks a record when control records arrive, and, for a
//! record of a removed author, when channel records of its channel arrive (a
//! missing cut head or a new witness can change its verdict).
//!
//! Pruned stubs (B6-p): once a delete or edit replaced a message or an
//! edit, peers may keep only its stub ([`Stub`]: every field but the body).
//! [`check_stub`] judges one like its full record, but only once a held,
//! accepted delete or edit replaced it; until then it is pending on its own
//! ID, and the caller fetches the full record, so a peer can't censor
//! messages by serving stubs. A stub never counts as a witness. Records
//! that name a stub (an edit or delete of a pruned message) find it held.
//!
//! Message text is encrypted on the wire, so its length can't be read here:
//! the caller checks the plaintext with [`message_text_ok`] after decrypting.
//!
//! Records only count in a live channel: created by the owner's
//! `ChannelCreate` and not deleted, at the record's `auth_deps`. A server
//! without channel records (made before #113) keeps a derived #general and
//! takes records in any channel, like `core/api`'s `known_channels`. 18+
//! channels are B7.

use std::collections::{BTreeSet, HashMap, HashSet};

use records::{Kind, PublicKey, Record, RecordId, Scope, ServerId, Stub};

use crate::{Action, Actor, ChannelStatus, Cut, Resolver, Role, View};

/// Most Unicode code points in a message's text, the same limit as the UI
/// (on top of the record's 16 KiB body limit).
pub const MAX_MESSAGE_CODE_POINTS: usize = 4000;

/// Most witnesses judged inside one another while judging one record. A
/// witness further down that chain doesn't count, which bounds the stack
/// for hostile chains of removed authors replying to each other.
pub const MAX_WITNESS_DEPTH: usize = 64;

/// Whether a message's plaintext is within [`MAX_MESSAGE_CODE_POINTS`].
/// Code points, not bytes or UTF-16 units.
pub fn message_text_ok(text: &str) -> bool {
    text.chars().count() <= MAX_MESSAGE_CODE_POINTS
}

/// What the caller's store holds of a server's channels.
pub trait ChannelGraph {
    /// Whether `ancestor` is `of`, or one of `of`'s ancestors through deps.
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool;
    /// A channel record the store holds in full.
    fn record(&self, id: RecordId) -> Option<Record>;
    /// A channel record the store holds, in full or as a pruned stub (B6-p).
    /// A store that keeps stubs overrides this; by default only full records
    /// are held.
    fn entry(&self, id: RecordId) -> Option<Entry> {
        self.record(id).map(Entry::Full)
    }
    /// The held records that list `id` in their deps.
    fn children(&self, id: RecordId) -> Vec<RecordId>;
}

/// A held channel record: in full, or pruned to a stub after a delete or
/// edit replaced it (B6-p).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Full(Record),
    Pruned(Stub),
}

impl Entry {
    pub fn id(&self) -> RecordId {
        match self {
            Entry::Full(record) => record.id(),
            Entry::Pruned(stub) => stub.id(),
        }
    }

    pub fn server(&self) -> ServerId {
        match self {
            Entry::Full(record) => record.server(),
            Entry::Pruned(stub) => stub.server(),
        }
    }

    pub fn scope(&self) -> Scope {
        match self {
            Entry::Full(record) => record.scope(),
            Entry::Pruned(stub) => stub.scope(),
        }
    }

    pub fn author(&self) -> PublicKey {
        match self {
            Entry::Full(record) => record.author(),
            Entry::Pruned(stub) => stub.author(),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Entry::Full(record) => record.kind(),
            Entry::Pruned(stub) => stub.kind(),
        }
    }

    pub fn deps(&self) -> &[RecordId] {
        match self {
            Entry::Full(record) => record.deps(),
            Entry::Pruned(stub) => stub.deps(),
        }
    }

    pub fn auth_deps(&self) -> &[RecordId] {
        match self {
            Entry::Full(record) => record.auth_deps(),
            Entry::Pruned(stub) => stub.auth_deps(),
        }
    }

    fn full(&self) -> Option<&Record> {
        match self {
            Entry::Full(record) => Some(record),
            Entry::Pruned(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Something it is judged by isn't held yet: an `auth_dep`, the target
    /// of an edit or delete, or a cut head of its channel. Check again when
    /// it arrives.
    Pending {
        missing: Vec<RecordId>,
    },
    Refused(Refusal),
    /// Voided by the accepted kick or revoke `by`.
    Voided {
        by: RecordId,
    },
    /// Rule B only: not voided, but the UI hides it (removed by `by`).
    Hidden {
        by: RecordId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Another server's record, or a control-log record.
    NotThisServersChannel,
    /// Not a channel kind.
    NotAChannelKind,
    /// A channel record must rely on at least one control-log head.
    NoAuthDeps,
    /// The author isn't a member (or is kicked) at its `auth_deps`.
    NotAMember,
    /// The author's role doesn't allow this kind at its `auth_deps`.
    NotAllowed,
    /// The channel isn't live at its `auth_deps`: never created, or deleted.
    NoSuchChannel,
    /// The owner deleted the server (`ServerClose`).
    ServerClosed,
    /// An edit or delete whose body doesn't name a target in its deps, or
    /// whose target isn't a `Message`.
    BadTarget,
}

/// How an accepted removal treats a concurrent channel record of its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Void {
    Keep,
    Void,
    Hide,
    /// Can't tell until these records are held.
    Pending(Vec<RecordId>),
}

/// A voiding rule: given the target's concurrent channel record, whether it
/// is privileged, and the removal, decide. `cx` answers questions about the
/// channel, witnesses included.
pub type VoidRule = fn(&Entry, bool, &Removal<'_>, &mut Context<'_>) -> Void;

/// An accepted kick or revoke, as a [`VoidRule`] sees it.
pub struct Removal<'a> {
    pub id: RecordId,
    pub kick: bool,
    pub cut: &'a Cut,
}

/// One check in progress: the control state, the caller's channels, the
/// rule, and the verdicts of witnesses judged so far.
pub struct Context<'a> {
    resolver: &'a Resolver,
    graph: &'a dyn ChannelGraph,
    rule: VoidRule,
    memo: HashMap<RecordId, Verdict>,
    depth: usize,
    /// A witness check was cut off at [`MAX_WITNESS_DEPTH`] while judging the
    /// current record; such verdicts aren't memoized.
    truncated: bool,
}

impl Context<'_> {
    pub fn graph(&self) -> &dyn ChannelGraph {
        self.graph
    }

    /// Whether a record of another author that descends from `record`, and
    /// is concurrent with `removal` (none of its `auth_deps` has seen it), is
    /// accepted. A record built on `record` after the removal is known
    /// doesn't count, so a colluder can't revive a backdated record. A
    /// pruned stub is never a witness: what it said can't be checked.
    pub fn witnessed(&mut self, record: &Entry, removal: &Removal<'_>) -> bool {
        if self.depth >= MAX_WITNESS_DEPTH {
            self.truncated = true;
            return false;
        }
        let mut queue: BTreeSet<RecordId> = self.graph.children(record.id()).into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(id) = queue.pop_first() {
            if !seen.insert(id) {
                continue;
            }
            let Some(next) = self.graph.entry(id) else { continue };
            if matches!(next, Entry::Full(_))
                && next.author() != record.author()
                && !self.resolver.is_control_ancestor(removal.id, next.auth_deps())
            {
                self.depth += 1;
                let accepted = self.judge(&next) == Verdict::Accept;
                self.depth -= 1;
                if accepted {
                    return true;
                }
            }
            queue.extend(self.graph.children(id));
        }
        false
    }

    fn judge(&mut self, record: &Entry) -> Verdict {
        if let Some(verdict) = self.memo.get(&record.id()) {
            return verdict.clone();
        }
        let outer = std::mem::take(&mut self.truncated);
        let verdict = self.check(record);
        if !self.truncated {
            self.memo.insert(record.id(), verdict.clone());
        }
        self.truncated |= outer;
        verdict
    }

    /// Whether a held, accepted delete or edit replaced the pruned `stub`:
    /// one naming it, for a message; for an edit, one naming the message it
    /// edited (among its deps, by the same author), since edits and deletes
    /// only ever name messages.
    fn replaced(&mut self, stub: &Entry) -> bool {
        let named = match stub.kind() {
            Kind::Message => vec![stub.id()],
            Kind::MessageEdit => stub
                .deps()
                .iter()
                .copied()
                .filter(|dep| {
                    self.graph.entry(*dep).is_some_and(|message| {
                        message.kind() == Kind::Message && message.author() == stub.author()
                    })
                })
                .collect(),
            _ => return false,
        };
        named.into_iter().any(|message| {
            self.graph.children(message).into_iter().any(|id| {
                let Some(Entry::Full(tombstone)) = self.graph.entry(id) else { return false };
                let kind = tombstone.kind();
                // A newer edit replaces an older one; a delete replaces both.
                let replaces = kind == Kind::MessageDelete
                    || (kind == Kind::MessageEdit
                        && (stub.kind() == Kind::Message
                            || self.graph.is_ancestor(stub.id(), tombstone.id())));
                replaces
                    && target_of(&tombstone, kind) == Some(message)
                    && tombstone.id() != stub.id()
                    && self.judge(&Entry::Full(tombstone)) == Verdict::Accept
            })
        })
    }

    fn check(&mut self, record: &Entry) -> Verdict {
        let resolver = self.resolver;
        if record.server() != resolver.server() || !matches!(record.scope(), Scope::Channel(_)) {
            return Verdict::Refused(Refusal::NotThisServersChannel);
        }
        // A stub stands in for its record only once something accepted
        // replaced it; otherwise the full record is fetched, so a peer can't
        // censor messages by serving stubs.
        if matches!(record, Entry::Pruned(_)) && !self.replaced(record) {
            return Verdict::Pending { missing: vec![record.id()] };
        }
        // A deleted server takes no channel records: its peers purge it.
        if resolver.state().closed.is_some() {
            return Verdict::Refused(Refusal::ServerClosed);
        }
        if record.auth_deps().is_empty() {
            return Verdict::Refused(Refusal::NoAuthDeps);
        }
        let at = match resolver.position_after(record.auth_deps()) {
            Ok(at) => at,
            Err(missing) => return Verdict::Pending { missing },
        };
        let view = resolver.view_at(at);
        let author = record.author();
        if view.is_kicked(author) || !view.is_member(author) {
            return Verdict::Refused(Refusal::NotAMember);
        }
        // Only live channels take records: created and not deleted. A server
        // without channel records (made before #113) keeps a derived #general.
        if let Scope::Channel(channel) = record.scope() {
            if view.has_channels() && !matches!(view.channel(channel), ChannelStatus::Live { .. }) {
                return Verdict::Refused(Refusal::NoSuchChannel);
            }
        }

        let privileged = match (record.kind(), record.full()) {
            (Kind::Message, _) => false,
            // A pruned edit: its target went with the body. Something
            // accepted replaced it, and an edit is its author's own.
            (Kind::MessageEdit, None) => false,
            (Kind::KeyRotation, _) => {
                if !may_rotate(&view, author) {
                    return Verdict::Refused(Refusal::NotAllowed);
                }
                true
            }
            (kind @ (Kind::MessageEdit | Kind::MessageDelete), Some(full)) => {
                let Some(target) = target_of(full, kind) else {
                    return Verdict::Refused(Refusal::BadTarget);
                };
                let Some(target) = self.graph.entry(target) else {
                    return Verdict::Pending { missing: vec![target] };
                };
                if target.kind() != Kind::Message || target.scope() != record.scope() {
                    return Verdict::Refused(Refusal::BadTarget);
                }
                let owner = target.author();
                if owner == author {
                    false
                } else if kind == Kind::MessageDelete
                    && view.role_of(author) >= Role::Moderator
                    && view.role_of(author) > view.role_of(owner)
                {
                    // Deleting someone else's message is a moderator's power.
                    true
                } else {
                    return Verdict::Refused(Refusal::NotAllowed);
                }
            }
            _ => return Verdict::Refused(Refusal::NotAChannelKind),
        };

        let mut hidden = None;
        for (id, action) in resolver.removals_of(author) {
            // A record whose auth_deps had seen the removal was judged with it.
            if resolver.is_control_ancestor(id, record.auth_deps()) {
                continue;
            }
            let (kick, cut) = match action {
                Action::Kick { cut, .. } => (true, cut),
                Action::Revoke { cut, .. } => (false, cut),
                _ => continue,
            };
            match (self.rule)(record, privileged, &Removal { id, kick, cut }, self) {
                Void::Keep => {}
                Void::Void => return Verdict::Voided { by: id },
                Void::Hide => hidden = hidden.or(Some(id)),
                Void::Pending(missing) => return Verdict::Pending { missing },
            }
        }
        match hidden {
            Some(by) => Verdict::Hidden { by },
            None => Verdict::Accept,
        }
    }
}

/// Rule A, the witness (default): the cut names the heads the remover had
/// seen in each channel. In a channel the cut names, a record of the target
/// that isn't an ancestor of one of that channel's heads is voided: by a
/// kick, unless it is non-privileged and witnessed (see
/// [`Context::witnessed`]), so a kick on a stale cut doesn't erase what
/// others had already replied to; by a revoke, only if it is privileged.
/// Privileged records keep the plain cut rule: a witness can't save a
/// backdated rotation. A channel the cut doesn't name is not voided at all.
/// Only a record that nothing keeps waits on a missing head: if a head the
/// cut names for its channel isn't held, the verdict is pending rather than
/// void. A fake head can't hold back records a held head or a witness keeps.
pub fn witness(
    record: &Entry,
    privileged: bool,
    removal: &Removal<'_>,
    cx: &mut Context<'_>,
) -> Void {
    if !removal.kick && !privileged {
        return Void::Keep;
    }
    let Scope::Channel(channel) = record.scope() else {
        return Void::Keep;
    };
    let heads: Vec<RecordId> = removal.cut.heads_of(channel).collect();
    if heads.is_empty() {
        return Void::Keep;
    }
    // Keep first, then wait: a head the caller doesn't hold (or a fake one)
    // only matters for a record nothing else saves.
    if heads.iter().any(|head| cx.graph().is_ancestor(record.id(), *head))
        || (!privileged && cx.witnessed(record, removal))
    {
        return Void::Keep;
    }
    let missing: Vec<RecordId> =
        heads.iter().copied().filter(|head| cx.graph().entry(*head).is_none()).collect();
    if missing.is_empty() {
        Void::Void
    } else {
        Void::Pending(missing)
    }
}

/// Rule B: the cut is ignored. Concurrent privileged records are voided;
/// a kick hides the target's other concurrent records instead of voiding
/// them.
pub fn privileged_only(
    _: &Entry,
    privileged: bool,
    removal: &Removal<'_>,
    _: &mut Context<'_>,
) -> Void {
    match (privileged, removal.kick) {
        (true, _) => Void::Void,
        (false, true) => Void::Hide,
        (false, false) => Void::Keep,
    }
}

/// Whether `actor` may rotate a channel's key at this point: a member with
/// the moderator role or above.
pub(crate) fn may_rotate(view: &impl View, actor: Actor) -> bool {
    view.is_member(actor) && !view.is_kicked(actor) && view.role_of(actor) >= Role::Moderator
}

/// Judges a channel record with the default rule, [`witness`].
pub fn check_channel<G: ChannelGraph>(resolver: &Resolver, record: &Record, graph: &G) -> Verdict {
    check_channel_with(resolver, record, graph, witness)
}

/// Judges a channel record against `resolver`'s control state, voiding by
/// `rule`.
pub fn check_channel_with<G: ChannelGraph>(
    resolver: &Resolver,
    record: &Record,
    graph: &G,
    rule: VoidRule,
) -> Verdict {
    check_entry(resolver, &Entry::Full(record.clone()), graph, rule)
}

/// Judges a pruned stub with the default rule: like its full record, once a
/// held, accepted delete or edit replaced it; `Pending` on its own ID until
/// then, meaning fetch the full record.
pub fn check_stub<G: ChannelGraph>(resolver: &Resolver, stub: &Stub, graph: &G) -> Verdict {
    check_entry(resolver, &Entry::Pruned(stub.clone()), graph, witness)
}

fn check_entry<G: ChannelGraph>(
    resolver: &Resolver,
    entry: &Entry,
    graph: &G,
    rule: VoidRule,
) -> Verdict {
    let mut cx =
        Context { resolver, graph, rule, memo: HashMap::new(), depth: 0, truncated: false };
    cx.judge(entry)
}

/// The record an edit or delete names: the first 32 bytes of the body (the
/// whole body for a delete), which must also be one of its deps.
fn target_of(record: &Record, kind: Kind) -> Option<RecordId> {
    let body = record.body();
    let (id, rest) = body.split_first_chunk::<32>()?;
    if kind == Kind::MessageDelete && !rest.is_empty() {
        return None;
    }
    let id = RecordId::from_bytes(*id);
    record.deps().contains(&id).then_some(id)
}
