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
//!   Edits and deletes name their target, which must be in their deps.
//! - A record that passes can still be voided by an accepted kick or revoke
//!   of its author that it is concurrent with, that is, none of its
//!   `auth_deps` has the removal as an ancestor. Which records that voids is
//!   the [`VoidRule`], swappable until the owner decides between
//!   [`witness`] (A, the default) and [`privileged_only`] (B).
//!   Under A, a record another member has accepted a reply to (a witness)
//!   survives a kick on a stale cut; witnesses depend on verdicts, so the
//!   caller re-checks until they settle (see [`ChannelGraph::witnessed`]).
//!
//! Message text is encrypted on the wire, so its length can't be read here:
//! the caller checks the plaintext with [`message_text_ok`] after decrypting.
//!
//! Channel existence, deletion and 18+ channels come with the channel
//! records (B4-3).

use records::{Kind, Record, RecordId, Scope};

use crate::{Action, Actor, Cut, Resolver, Role, View};

/// Most Unicode code points in a message's text, the same limit as the UI
/// (on top of the record's 16 KiB body limit).
pub const MAX_MESSAGE_CODE_POINTS: usize = 4000;

/// Whether a message's plaintext is within [`MAX_MESSAGE_CODE_POINTS`].
/// Code points, not bytes or UTF-16 units.
pub fn message_text_ok(text: &str) -> bool {
    text.chars().count() <= MAX_MESSAGE_CODE_POINTS
}

/// What the caller's store knows about a channel's records.
pub trait ChannelGraph {
    /// Whether `ancestor` is `of`, or one of `of`'s ancestors through deps.
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool;
    /// The author of a channel record the store holds.
    fn author(&self, id: RecordId) -> Option<Actor>;
    /// Whether a record the caller has **accepted** (verdict `Accept`), by
    /// someone other than `author`, has `record` as an ancestor: another
    /// member saw it before replying.
    ///
    /// Acceptance depends on these verdicts in turn, so the caller re-checks
    /// until nothing changes: re-judge the records of a removed author
    /// whenever a verdict in their channel changes, and repeat while any
    /// verdict flips. It ends: a record keeps its witnesses as long as they
    /// stay accepted, and verdicts only change when a witness is voided.
    fn witnessed(&self, record: RecordId, author: Actor) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Some `auth_deps` haven't joined the control log (or, for an edit or
    /// delete, the target isn't held). Check again when they arrive.
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
    /// that targets a record of another channel kind it can't change.
    BadTarget,
}

/// How an accepted removal treats a concurrent channel record of its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Void {
    Keep,
    Void,
    Hide,
}

/// A voiding rule: given the target's concurrent channel record, whether it
/// is privileged, the removal and its kind, decide.
pub type VoidRule = fn(&Record, bool, &Removal<'_>, &dyn ChannelGraph) -> Void;

/// An accepted kick or revoke, as a [`VoidRule`] sees it.
pub struct Removal<'a> {
    pub id: RecordId,
    pub kick: bool,
    pub cut: &'a Cut,
}

/// Rule A, the witness (default): the cut names the heads the remover had
/// seen in each channel. In a channel the cut names, a record of the target
/// that isn't an ancestor of one of that channel's heads is voided: by a
/// kick, unless it is non-privileged and **witnessed** (an accepted record
/// by another author has it as an ancestor, see
/// [`ChannelGraph::witnessed`]), so a kick on a stale cut doesn't erase
/// what others already replied to; by a revoke, only if it is privileged.
/// Privileged records keep the plain cut rule: a witness can't save a
/// backdated rotation. A channel the cut doesn't name is not voided at all.
pub fn witness(
    record: &Record,
    privileged: bool,
    removal: &Removal<'_>,
    graph: &dyn ChannelGraph,
) -> Void {
    if !removal.kick && !privileged {
        return Void::Keep;
    }
    let Scope::Channel(channel) = record.scope() else {
        return Void::Keep;
    };
    let mut heads = removal.cut.heads_of(channel).peekable();
    if heads.peek().is_none() {
        return Void::Keep;
    }
    if heads.any(|head| graph.is_ancestor(record.id(), head))
        || (!privileged && graph.witnessed(record.id(), record.author()))
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
    _: &dyn ChannelGraph,
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
pub fn check_channel(resolver: &Resolver, record: &Record, graph: &impl ChannelGraph) -> Verdict {
    check_channel_with(resolver, record, graph, witness)
}

/// Judges a channel record against `resolver`'s control state, voiding by
/// `rule`.
pub fn check_channel_with(
    resolver: &Resolver,
    record: &Record,
    graph: &impl ChannelGraph,
    rule: VoidRule,
) -> Verdict {
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
            let Some(owner) = graph.author(target) else {
                return Verdict::Pending { missing: vec![target] };
            };
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
        match rule(record, privileged, &Removal { id, kick, cut }, graph) {
            Void::Keep => {}
            Void::Void => return Verdict::Voided { by: id },
            Void::Hide => hidden = hidden.or(Some(id)),
        }
    }
    match hidden {
        Some(by) => Verdict::Hidden { by },
        None => Verdict::Accept,
    }
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
