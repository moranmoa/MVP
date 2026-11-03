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
//! 4. The [`Authorizer`] (the integration layer, using `core/authority`)
//!    says the author may write it. Roles come only from the signed log.
//!
//! Only then is a record stored, forwarded on gossip or served to others.
//!
//! - [`Store`]: SQLite store with `append`, `get`, `heads`, `missing_since`.
//! - [`node`]: a libp2p node that finds peers through the bootstrap/relay
//!   node, pulls what it is missing (heads + markers → missing records, paged)
//!   and pushes new records live over gossipsub.

pub mod node;
pub mod store;

mod limits;
#[cfg(test)]
pub(crate) mod testutil;

pub use node::{Node, NodeConfig, NodeEvent, Via};
pub use records::{ChannelId, Record, RecordId, Scope, ServerId};
pub use store::{Append, IngestError, Store};

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

/// Whether the author of a record may write it, judged from the signed
/// control log only (`core/authority`). Called after the record decoded and
/// all its `deps` and `auth_deps` are stored, before it is stored itself or
/// forwarded. Implemented by the integration layer (T1).
pub trait Authorizer: Send + Sync + 'static {
    fn check(&self, record: &Record) -> Verdict;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Not stored and not forwarded; gossip reports it as invalid.
    Reject,
}

/// Accepts every validly signed record. For the harness and tests only: it
/// lets anyone who knows a log's name write to it.
pub struct AcceptAll;

impl Authorizer for AcceptAll {
    fn check(&self, _: &Record) -> Verdict {
        Verdict::Accept
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
