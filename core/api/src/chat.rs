//! Channel messages: judged by `authority` against the control log, sealed
//! and opened through the [`GroupCipher`].
//!
//! - Every channel record is judged with `authority::check_channel` over the
//!   channel records held ([`Graph`]). Only `Accept` counts: pending, refused
//!   and voided records are neither shown nor used.
//! - Every `KeyRotation` record goes to `apply_rotation` once (accepted or
//!   not); the accepted ones go to `set_accepted` (deny by default, GAP-5).
//! - An accepted `Message` is decrypted; a body that doesn't open, or opens
//!   under a rotation authority doesn't accept (`Opened.accepted == false`,
//!   #21), or whose text is over `message_text_ok`, isn't shown.
//!
//! TODO(#21): show untrusted messages as such instead of hiding them.
//! TODO(groupkeys): re-request the part records `apply_rotation` evicted.
//! TODO(B4-4): re-check pending records when what they wait for arrives
//! (today every read judges again from scratch).

use std::collections::{BTreeSet, HashMap, HashSet};

use authority::{check_channel, message_text_ok, ChannelGraph, Resolver, Verdict};
use records::{ChannelId, Kind, PublicKey, Record, RecordId, Scope, ServerId, MAX_DEPS};

use crate::model::Log;
use crate::ports::{GroupCipher, Opened};

/// The channel records of one server, as `authority` asks about them.
pub(crate) struct Graph<'a> {
    by_id: HashMap<RecordId, &'a Record>,
    children: HashMap<RecordId, Vec<RecordId>>,
}

impl<'a> Graph<'a> {
    pub fn new(records: &'a [Record]) -> Graph<'a> {
        let mut children: HashMap<RecordId, Vec<RecordId>> = HashMap::new();
        for record in records {
            for dep in record.deps() {
                children.entry(*dep).or_default().push(record.id());
            }
        }
        Graph { by_id: records.iter().map(|r| (r.id(), r)).collect(), children }
    }
}

impl ChannelGraph for Graph<'_> {
    fn is_ancestor(&self, ancestor: RecordId, of: RecordId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![of];
        while let Some(id) = stack.pop() {
            if id == ancestor {
                return true;
            }
            if seen.insert(id) {
                if let Some(record) = self.by_id.get(&id) {
                    stack.extend(record.deps().iter().copied());
                }
            }
        }
        false
    }

    fn record(&self, id: RecordId) -> Option<Record> {
        self.by_id.get(&id).map(|r| (*r).clone())
    }

    fn children(&self, id: RecordId) -> Vec<RecordId> {
        self.children.get(&id).cloned().unwrap_or_default()
    }
}

/// The records of `channel`, in the order they were added.
pub(crate) fn channel_records(log: &Log, channel: ChannelId) -> Vec<Record> {
    log.channel.iter().filter(|r| r.scope() == Scope::Channel(channel)).cloned().collect()
}

fn accepted(resolver: &Resolver, record: &Record, graph: &Graph) -> bool {
    check_channel(resolver, record, graph) == Verdict::Accept
}

/// The channels authority knows for `server`: only the default one until
/// channel records arrive (B4-3). Nothing about any other channel ever
/// reaches the cipher, whatever records name it.
pub(crate) fn known_channels(server: ServerId) -> BTreeSet<ChannelId> {
    BTreeSet::from([crate::model::default_channel(server)])
}

fn in_known(record: &Record, known: &BTreeSet<ChannelId>) -> bool {
    matches!(record.scope(), Scope::Channel(c) if known.contains(&c))
}

/// Brings the cipher up to date for `server`, in the order it needs:
/// the known channels, then the accepted rotations, then every stored
/// rotation of those channels. Done before every read and send, so an
/// unlock (which starts from nothing) or a lock in between loses nothing;
/// a rotation it already holds is a no-op (`Applied::Duplicate`).
/// TODO(perf): replay only after an unlock, once the core drives it.
pub(crate) fn sync_cipher(cipher: &dyn GroupCipher, server: ServerId, log: &Log) {
    let known = known_channels(server);
    cipher.set_channels(server, &known);
    let graph = Graph::new(&log.channel);
    let rotations: Vec<&Record> = log
        .channel
        .iter()
        .filter(|r| r.kind() == Kind::KeyRotation && in_known(r, &known))
        .collect();
    let ok: BTreeSet<RecordId> =
        rotations.iter().filter(|r| accepted(&log.resolver, r, &graph)).map(|r| r.id()).collect();
    cipher.set_accepted(server, &ok);
    for record in rotations {
        // An error is a rotation we can't use; nothing else to do.
        let _ = cipher.apply_rotation(record);
    }
}

/// A shown message: its record and opened text.
pub(crate) struct Shown {
    pub record: Record,
    /// The latest accepted edit's text, else the original; empty once deleted.
    pub text: String,
    /// The claimed time of the edit shown, if any.
    pub edited_at: Option<u64>,
    /// A tombstone: the message keeps its place, its text is gone.
    pub deleted: bool,
}

/// Opens a message or edit body; `None` unless it opens under an accepted
/// rotation (#21) as text within `message_text_ok`.
fn open(
    cipher: &dyn GroupCipher,
    server: ServerId,
    channel: ChannelId,
    author: PublicKey,
    body: &[u8],
) -> Option<String> {
    match cipher.decrypt(server, channel, author, body) {
        Ok(Opened { plaintext, accepted: true }) => {
            let text = String::from_utf8(plaintext).ok()?;
            message_text_ok(&text).then_some(text)
        }
        _ => None,
    }
}

/// An edit's or delete's target: the first 32 bytes of its body (authority
/// has checked it names a `Message` in its deps).
pub(crate) fn target(record: &Record) -> Option<RecordId> {
    record.body().first_chunk::<32>().map(|id| RecordId::from_bytes(*id))
}

/// The accepted messages of `channel` that open, oldest first by their
/// claimed time, then ID.
pub(crate) fn messages(
    cipher: &dyn GroupCipher,
    server: ServerId,
    channel: ChannelId,
    log: &Log,
) -> Vec<Shown> {
    if !known_channels(server).contains(&channel) {
        return Vec::new();
    }
    let records = channel_records(log, channel);
    let graph = Graph::new(&log.channel);
    let ok: Vec<&Record> = records.iter().filter(|r| accepted(&log.resolver, r, &graph)).collect();
    let deleted: HashSet<RecordId> =
        ok.iter().filter(|r| r.kind() == Kind::MessageDelete).filter_map(|r| target(r)).collect();
    // The latest accepted edit per message, by claimed time then ID.
    let mut edits: HashMap<RecordId, &Record> = HashMap::new();
    for edit in ok.iter().filter(|r| r.kind() == Kind::MessageEdit) {
        let Some(of) = target(edit) else { continue };
        let newer = edits.get(&of).is_none_or(|e| (e.ts(), e.id()) < (edit.ts(), edit.id()));
        if newer {
            edits.insert(of, edit);
        }
    }
    let mut shown: Vec<Shown> = ok
        .iter()
        .filter(|r| r.kind() == Kind::Message)
        .filter_map(|record| {
            let original = open(cipher, server, channel, record.author(), record.body())?;
            let record = (*record).clone();
            if deleted.contains(&record.id()) {
                return Some(Shown { record, text: String::new(), edited_at: None, deleted: true });
            }
            let edit = edits.get(&record.id()).and_then(|e| {
                let text = open(cipher, server, channel, e.author(), e.body().get(32..)?)?;
                Some((text, e.ts()))
            });
            Some(match edit {
                Some((text, at)) => Shown { record, text, edited_at: Some(at), deleted: false },
                None => Shown { record, text: original, edited_at: None, deleted: false },
            })
        })
        .collect();
    shown.sort_by_key(|s| (s.record.ts(), s.record.id()));
    shown
}

/// Deps for a new record in `channel`: its records no other one depends on,
/// the latest [`MAX_DEPS`], sorted as `records` requires.
pub(crate) fn channel_deps(log: &Log, channel: ChannelId) -> Vec<RecordId> {
    let records = channel_records(log, channel);
    let referenced: HashSet<RecordId> =
        records.iter().flat_map(|r| r.deps().iter().copied()).collect();
    let heads: Vec<RecordId> =
        records.iter().map(Record::id).filter(|id| !referenced.contains(id)).collect();
    let mut deps = heads[heads.len().saturating_sub(MAX_DEPS)..].to_vec();
    deps.sort();
    deps
}

/// The next `seq` for `author` in `channel`.
pub(crate) fn channel_seq(log: &Log, channel: ChannelId, author: &PublicKey) -> u64 {
    channel_records(log, channel)
        .iter()
        .filter(|r| r.author() == *author)
        .map(|r| r.seq().saturating_add(1))
        .max()
        .unwrap_or(0)
}

/// Whether `authority` would accept `record` next to the records held: a
/// dry run, so a refused edit or delete is never stored or sent.
pub(crate) fn would_accept(log: &Log, record: &Record) -> bool {
    let mut records = log.channel.clone();
    records.push(record.clone());
    accepted(&log.resolver, record, &Graph::new(&records))
}

/// [`channel_deps`] plus `target`, which an edit or delete must list.
pub(crate) fn deps_with(log: &Log, channel: ChannelId, target: RecordId) -> Vec<RecordId> {
    let mut deps: Vec<RecordId> =
        channel_deps(log, channel).into_iter().filter(|d| *d != target).collect();
    if deps.len() >= MAX_DEPS {
        deps.remove(0);
    }
    deps.push(target);
    deps.sort();
    deps
}
