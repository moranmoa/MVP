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
//! Message text is encrypted on the wire, so its length can't be read here:
//! the caller checks the plaintext with [`message_text_ok`] after decrypting.
//!
//! Channel existence, deletion and 18+ channels come with the channel
//! records (B4-3).

use std::collections::{BTreeSet, HashMap, HashSet};

use records::{Kind, Record, RecordId, Scope};

use crate::{Action, Actor, Cut, Resolver, Role, View};

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
    /// A channel record the store holds.
    fn record(&self, id: RecordId) -> Option<Record>;
    /// The held records that list `id` in their deps.
    fn children(&self, id: RecordId) -> Vec<RecordId>;
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
pub type VoidRule = fn(&Record, bool, &Removal<'_>, &mut Context<'_>) -> Void;

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
}

impl Context<'_> {
    pub fn graph(&self) -> &dyn ChannelGraph {
        self.graph
    }

    /// Whether a record of another author that descends from `record`, and
    /// is concurrent with `removal` (none of its `auth_deps` has seen it), is
    /// accepted. A record built on `record` after the removal is known
    /// doesn't count, so a colluder can't revive a backdated record.
    pub fn witnessed(&mut self, record: &Record, removal: &Removal<'_>) -> bool {
        if self.depth >= MAX_WITNESS_DEPTH {
            return false;
        }
        let mut queue: BTreeSet<RecordId> = self.graph.children(record.id()).into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(id) = queue.pop_first() {
            if !seen.insert(id) {
                continue;
            }
            let Some(next) = self.graph.record(id) else { continue };
            if next.author() != record.author()
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

    fn judge(&mut self, record: &Record) -> Verdict {
        if let Some(verdict) = self.memo.get(&record.id()) {
            return verdict.clone();
        }
        let verdict = self.check(record);
        self.memo.insert(record.id(), verdict.clone());
        verdict
    }

    fn check(&mut self, record: &Record) -> Verdict {
        let resolver = self.resolver;
        if record.server() != resolver.server() || !matches!(record.scope(), Scope::Channel(_)) {
            return Verdict::Refused(Refusal::NotThisServersChannel);
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

        let privileged = match record.kind() {
            Kind::Message => false,
            Kind::KeyRotation => {
                if !may_rotate(&view, author) {
                    return Verdict::Refused(Refusal::NotAllowed);
                }
                true
            }
            kind @ (Kind::MessageEdit | Kind::MessageDelete) => {
                let Some(target) = target_of(record, kind) else {
                    return Verdict::Refused(Refusal::BadTarget);
                };
                let Some(target) = self.graph.record(target) else {
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
/// While a head the cut names for the channel isn't held, the answer is
/// pending rather than void.
pub fn witness(
    record: &Record,
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
    let missing: Vec<RecordId> =
        heads.iter().copied().filter(|head| cx.graph().record(*head).is_none()).collect();
    if !missing.is_empty() {
        return Void::Pending(missing);
    }
    if heads.iter().any(|head| cx.graph().is_ancestor(record.id(), *head))
        || (!privileged && cx.witnessed(record, removal))
    {
        Void::Keep
    } else {
        Void::Void
    }
}

/// Rule B: the cut is ignored. Concurrent privileged records are voided;
/// a kick hides the target's other concurrent records instead of voiding
/// them.
pub fn privileged_only(
    _: &Record,
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
    let mut cx = Context { resolver, graph, rule, memo: HashMap::new(), depth: 0 };
    cx.judge(record)
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
