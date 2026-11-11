//! Log sync between peers.
//!
//! What travels and what is stored are signed records (`core/records`), and
//! nothing a peer sends is trusted until it has passed, in this order:
//!
//! 1. [`records::Record::decode`]: size, canonical form, signature. The record
//!    id is computed from the signed bytes, never taken from the wire.
//! 2. The log it is filed under is derived from the record ([`log_of`]),
//!    never from the gossip topic or the pull request.
//! 3. `deps` are in that same log and `auth_deps` in the same server's
//!    control log, and all of them are present ([`Store`]).
//! 4. For channel records only, the [`Authorizer`] (the integration layer,
//!    using `core/authority`) says the author may write it. Roles come only
//!    from the signed log. Control records are never refused at ingest:
//!    authority is a function of the whole set, which must not depend on the
//!    order records arrived in; authority decides what each one means.
//!
//! Only then is a record stored, forwarded on gossip or served to others.
//!
//! - [`Store`]: SQLite store with `append`, `get`, `heads`, `missing_since`.
//! - [`node`]: a libp2p node that finds peers through the bootstrap/relay
//!   node, pulls what it is missing (heads + markers → missing records, paged)
//!   and pushes new records live over gossipsub.

pub mod access;
pub mod node;
pub mod store;

mod announce;
mod keyfile;
mod limits;
mod presence;
mod punch;
mod backup;
mod invite;
mod voice;
#[cfg(test)]
pub(crate) mod testutil;

pub use access::{binding_message, Binder, Handle, ReadGate, ServerSecret};
pub use node::{Access, Node, NodeConfig, NodeEvent, PresenceState, Reachability, Via};
pub use records::{ChannelId, Record, RecordId, Scope, ServerId, Stub};
pub use backup::MAX_BACKUP;
pub use invite::{MAX_PER_SERVER as MAX_INVITES_PER_SERVER, MAX_PUBLICATION as MAX_INVITE_PUBLICATION};
pub use voice::{VoiceError, MAX_VOICE_BLOB};
pub use store::{Append, ChannelLog, ControlLog, Held, IngestError, Store};

/// Longest log name, in bytes. A channel log name is 64 + 1 + 64 characters.
pub const MAX_LOG_NAME: usize = 160;

/// The log a record belongs to, derived from what it signed: the server's
/// control log is `<server>/control`, a channel's log is `<server>/<channel>`
/// (lowercase hex ids).
pub fn log_name(server: ServerId, scope: Scope) -> String {
    match scope {
        Scope::Control => format!("{server}/control"),
        Scope::Channel(channel) => format!("{server}/{channel}"),
    }
}

/// The log `record` must be filed under.
pub fn log_of(record: &Record) -> String {
    log_name(record.server(), record.scope())
}

/// A log name: 1–160 printable ASCII characters, no spaces. Log names go into
/// gossipsub topic names and SQL parameters (never into SQL text).
pub fn valid_log_name(log: &str) -> bool {
    !log.is_empty() && log.len() <= MAX_LOG_NAME && log.bytes().all(|b| b.is_ascii_graphic())
}

/// The **storage gate** for channel records: may this record be stored and
/// served? Called after the record decoded and all its `deps` and
/// `auth_deps` are stored, with read access to the server's stored control
/// log. Never called for control records (see the crate docs). Implemented
/// by the integration layer (T1).
///
/// **The verdict must be monotone in the stored control log:** if it accepts
/// a record while the control log holds the set S, it must accept it for
/// every superset of S. Rules of the form "the author is the owner, or the
/// author has a stored `Join` (whose invite is stored)" qualify; so does
/// "a stored `Grant` names the author".
///
/// Kicks, revokes, voided records and the GAP-2 `auth_deps` checks must
/// **not** be used here (so not `authority::check_channel`): they can flip a
/// verdict when a record arrives late. A gate that does that leaves honest
/// children of a refused record waiting forever and splits the channel DAG
/// between peers that judged it at different times. The full authority
/// verdict belongs to display, forwarding to the app, and keys; it never
/// deletes anything from the store. Spam is bounded by per-author quotas
/// (GAP-8) and members-only transport (GAP-4), not by this gate.
///
/// `check` runs inside the store's write transaction on the node's task: it
/// must not call back into the node.
pub trait Authorizer: Send + Sync + 'static {
    fn check(&self, record: &Record, control: &ControlLog<'_>) -> Verdict;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Not stored, served or forwarded (gossip: Ignore, since a peer with
    /// more of the control log may rightly accept it). May change to Accept
    /// once more control records are stored.
    Reject,
}

/// Accepts every validly signed record. For the harness and tests only: it
/// lets anyone who knows a log's name write to it.
pub struct AcceptAll;

impl Authorizer for AcceptAll {
    fn check(&self, _: &Record, _: &ControlLog<'_>) -> Verdict {
        Verdict::Accept
    }
}

/// Decides about pruned stubs (B6-p): whether a peer's stub may be stored
/// in place of its record, and whether we may prune one of ours. Implemented
/// by the integration layer over `authority::check_stub`. The stub is held
/// only once a held, accepted `MessageDelete` (or newer `MessageEdit`)
/// replaced its record; until then it is [`StubVerdict::Pending`]. So a
/// hostile peer serving stubs can't make messages disappear.
///
/// Called with the stub's context (deps, auth_deps) stored, inside the
/// store's write transaction: it must not call back into the node.
pub trait StubGate: Send + Sync + 'static {
    fn check(&self, stub: &Stub, channel: &ChannelLog<'_>) -> StubVerdict;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StubVerdict {
    /// Store (or prune to) the stub.
    Accept,
    /// Nothing held replaced the record yet: not stored. Asked again when
    /// more of the channel arrives; meanwhile the full record may come.
    Pending,
    /// Never store it.
    Reject,
}

/// Refuses every stub: only full records are stored and served. The
/// default until the integration layer provides a gate.
pub struct NoStubs;

impl StubGate for NoStubs {
    fn check(&self, _: &Stub, _: &ChannelLog<'_>) -> StubVerdict {
        StubVerdict::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{channel, message, server};

    #[test]
    fn log_names_come_from_the_record() {
        let control = log_name(server(), Scope::Control);
        let chan = log_name(server(), Scope::Channel(channel()));
        assert!(control.ends_with("/control") && valid_log_name(&control));
        assert!(valid_log_name(&chan) && chan.len() == 64 + 1 + 64);
        assert_eq!(log_of(&message(0, &[], 0)), chan);
    }

    #[test]
    fn log_name_rules() {
        assert!(valid_log_name("general"));
        assert!(!valid_log_name(""));
        assert!(!valid_log_name("has space"));
        assert!(!valid_log_name("קבוצה"));
        assert!(!valid_log_name(&"x".repeat(MAX_LOG_NAME + 1)));
    }
}
