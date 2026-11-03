//! SQLite store of signed records, grouped into logs.
//!
//! Only a [`Record`] can be appended, and holding one means `Record::decode`
//! (or signing) already checked its format and signature. The log is derived
//! from the record ([`log_of`]). A record is only accepted once all of its
//! deps are stored in the same log and all of its `auth_deps` in the server's
//! control log, so the local insertion order (`seq`) is always a topological
//! order, and "I have X" implies "I have every ancestor of X" (and everything
//! X's author relied on for permission).
//!
//! The stored bytes are the record's canonical encoding: pages are served
//! from them as they are, without re-encoding.

use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use records::{Kind, ServerId, Stub};

use crate::{log_name, log_of, Authorizer, Record, RecordId, Scope, StubGate, StubVerdict, Verdict};

/// Most `have` ids looked at per [`Store::missing_since`] call; the rest are
/// ignored. [`Store::have_markers`] never produces more.
pub const MAX_HAVE: usize = 64;
/// Most heads included in [`Store::have_markers`].
const MAX_HAVE_HEADS: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Append {
    /// Newly stored.
    Added,
    /// Already had it.
    Known,
    /// Not stored: these deps or auth_deps are not in the store yet.
    MissingDeps(Vec<RecordId>),
    /// Not stored, and never will be.
    Invalid(IngestError),
    /// Not stored: the [`Authorizer`] (or, for a stub, the [`StubGate`])
    /// refused it.
    Unauthorized,
    /// A stub not stored yet: nothing held replaced its record (B6-p). Try
    /// again once more of its channel arrives, or get the full record.
    StubPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestError {
    /// A dep is stored under another log. Pulls only send one log, so peers
    /// without that other log open could never store this record.
    DepInOtherLog,
    /// An auth_dep is stored somewhere other than this server's control log.
    AuthDepNotInControlLog,
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::DepInOtherLog => write!(f, "a dep belongs to another log"),
            IngestError::AuthDepNotInControlLog => write!(f, "an auth_dep is not in the server's control log"),
        }
    }
}

/// A stored record as served to peers: its id and canonical bytes, which
/// are a pruned [`Stub`]'s when `stub` is set (B6-p).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub id: RecordId,
    pub bytes: Vec<u8>,
    pub stub: bool,
}

/// What the store holds for an id: the full record, or its pruned stub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    Full(Record),
    Stub(Stub),
}

impl Held {
    pub fn id(&self) -> RecordId {
        match self {
            Held::Full(r) => r.id(),
            Held::Stub(s) => s.id(),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Held::Full(r) => r.kind(),
            Held::Stub(s) => s.kind(),
        }
    }

    fn decode(bytes: &[u8], stub: bool) -> Option<Held> {
        if stub {
            Stub::decode(bytes).ok().map(Held::Stub)
        } else {
            Record::decode(bytes).ok().map(Held::Full)
        }
    }
}

/// Read access to one channel log and its server's control log, for a
/// [`StubGate`] (the integration layer builds authority's `ChannelGraph` on
/// it).
pub struct ChannelLog<'a> {
    conn: &'a Connection,
    log: String,
    server: ServerId,
}

impl ChannelLog<'_> {
    /// What is held for `id`, if it is in this log.
    pub fn entry(&self, id: &RecordId) -> rusqlite::Result<Option<Held>> {
        let row: Option<(Vec<u8>, bool)> = self
            .conn
            .prepare_cached("SELECT data, stub FROM records WHERE id = ?1 AND log = ?2")?
            .query_row(params![&id.as_bytes()[..], self.log], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row.and_then(|(bytes, stub)| Held::decode(&bytes, stub)))
    }

    /// The held records that list `id` as a dep.
    pub fn children(&self, id: &RecordId) -> rusqlite::Result<Vec<Held>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.data, r.stub FROM deps d JOIN records r ON r.id = d.child WHERE d.parent = ?1 ORDER BY r.seq",
        )?;
        let rows = stmt.query_map([&id.as_bytes()[..]], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, bool>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (bytes, stub) = row?;
            out.extend(Held::decode(&bytes, stub));
        }
        Ok(out)
    }

    pub fn control(&self) -> ControlLog<'_> {
        ControlLog { conn: self.conn, log: log_name(self.server, Scope::Control) }
    }
}

pub struct Store {
    conn: Connection,
}

/// Read access to one server's stored control log, for an [`Authorizer`]
/// deciding about a channel record of that server.
pub struct ControlLog<'a> {
    conn: &'a Connection,
    log: String,
}

impl ControlLog<'_> {
    /// Every stored control record of the server, decoded, in the order
    /// they were stored (a topological order).
    pub fn records(&self) -> rusqlite::Result<Vec<Record>> {
        let mut stmt = self.conn.prepare_cached("SELECT data FROM records WHERE log = ?1 ORDER BY seq")?;
        let rows = stmt.query_map([&self.log], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for bytes in rows {
            // Stored bytes passed decode once; anything corrupt is skipped.
            if let Ok(record) = Record::decode(&bytes?) {
                out.push(record);
            }
        }
        Ok(out)
    }

    /// How many control records of the server are stored. Cheap: an index
    /// count, nothing is read or decoded (e.g. to cache `records()` and
    /// notice when the control log grew).
    pub fn count(&self) -> rusqlite::Result<u64> {
        self.conn
            .prepare_cached("SELECT COUNT(*) FROM records WHERE log = ?1")?
            .query_row([&self.log], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
    }
}

fn id_of(bytes: [u8; 32]) -> RecordId {
    RecordId::from_bytes(bytes)
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A second, read-only connection to a store another [`Store`] writes,
    /// for answering pulls off the network task (WAL lets it read while the
    /// writer writes). Only the read methods work on it.
    pub fn open_read_only(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
        Ok(Self { conn: Connection::open_with_flags(path, flags)? })
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS records (
                 seq  INTEGER PRIMARY KEY AUTOINCREMENT,
                 id   BLOB NOT NULL UNIQUE,
                 log  TEXT NOT NULL,
                 data BLOB NOT NULL,
                 stub INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS records_log ON records (log, seq);
             CREATE TABLE IF NOT EXISTS deps (
                 child  BLOB NOT NULL,
                 parent BLOB NOT NULL,
                 PRIMARY KEY (child, parent)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS heads (
                 log TEXT NOT NULL,
                 id  BLOB NOT NULL,
                 PRIMARY KEY (log, id)
             ) WITHOUT ROWID;",
        )?;
        // Stores made before stubs (B6-p) lack the column.
        let has_stub: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('records') WHERE name = 'stub'")?
            .exists([])?;
        if !has_stub {
            conn.execute_batch("ALTER TABLE records ADD COLUMN stub INTEGER NOT NULL DEFAULT 0")?;
        }
        Ok(Self { conn })
    }

    pub fn has(&self, id: &RecordId) -> rusqlite::Result<bool> {
        Ok(self.log_of_id(id)?.is_some())
    }

    fn log_of_id(&self, id: &RecordId) -> rusqlite::Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT log FROM records WHERE id = ?1")?;
        stmt.query_row([&id.as_bytes()[..]], |r| r.get(0)).optional()
    }

    /// What storing `record` would need: `Ok(missing)` lists absent deps and
    /// auth_deps (empty: it can be stored now), `Err` means it never can be.
    /// The node calls this before asking the [`crate::Authorizer`], which
    /// needs the record's whole context present.
    pub fn check(&self, record: &Record) -> rusqlite::Result<Result<Vec<RecordId>, IngestError>> {
        check_in(&self.conn, record)
    }

    /// Stores `record` if its context is present and, for channel records,
    /// `auth` accepts it (control records are never refused; see
    /// `append_in`). `auth` runs inside the write transaction: it must not
    /// call back into the node or this store.
    pub fn append(&mut self, record: &Record, auth: &dyn Authorizer) -> rusqlite::Result<Append> {
        let tx = self.conn.transaction()?;
        let result = append_in(&tx, record, auth)?;
        tx.commit()?;
        Ok(result)
    }

    /// Appends many records in one transaction (much faster than one by
    /// one). Returns what happened to each record, in order.
    pub fn append_all<'a>(
        &mut self,
        records: impl IntoIterator<Item = &'a Record>,
        auth: &dyn Authorizer,
    ) -> rusqlite::Result<Vec<Append>> {
        let tx = self.conn.transaction()?;
        let mut outcomes = Vec::new();
        for record in records {
            outcomes.push(append_in(&tx, record, auth)?);
        }
        tx.commit()?;
        Ok(outcomes)
    }

    /// The stored canonical bytes of a record.
    pub fn get(&self, id: &RecordId) -> rusqlite::Result<Option<Vec<u8>>> {
        self.conn.query_row("SELECT data FROM records WHERE id = ?1", [&id.as_bytes()[..]], |r| r.get(0)).optional()
    }

    /// A stored full record, decoded (and so re-verified). None for a stub.
    pub fn get_record(&self, id: &RecordId) -> rusqlite::Result<Option<Record>> {
        Ok(self.get(id)?.and_then(|bytes| Record::decode(&bytes).ok()))
    }

    /// What is held for `id`: the full record or its stub (for authority's
    /// `ChannelGraph::entry`).
    pub fn entry(&self, id: &RecordId) -> rusqlite::Result<Option<Held>> {
        let row: Option<(Vec<u8>, bool)> = self
            .conn
            .prepare_cached("SELECT data, stub FROM records WHERE id = ?1")?
            .query_row([&id.as_bytes()[..]], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row.and_then(|(bytes, stub)| Held::decode(&bytes, stub)))
    }

    /// Stores a peer's stub (B6-p) if its context is present and `gate`
    /// accepts it (something held replaced its record). Anything already
    /// held for its id, full record or stub, is kept: a peer's stub never
    /// replaces a full record here; only [`Store::prune`] does that.
    pub fn append_stub(&mut self, stub: &Stub, gate: &dyn StubGate) -> rusqlite::Result<Append> {
        let tx = self.conn.transaction()?;
        let result = append_stub_in(&tx, stub, gate)?;
        tx.commit()?;
        Ok(result)
    }

    /// Stores `record` together with pending stubs it depends on (B6-p): a
    /// delete or edit always depends on the message it replaces, so a
    /// newcomer that could only get the stub (everyone else pruned) would
    /// otherwise wait forever: the stub waits for the delete, the delete for
    /// the stub. In one transaction: the stubs go in, then the record (with
    /// the usual checks), then `gate` must accept every stub with the record
    /// held. Anything less, and nothing is stored.
    pub fn append_with_stubs(
        &mut self,
        record: &Record,
        stubs: &[Stub],
        auth: &dyn Authorizer,
        gate: &dyn StubGate,
    ) -> rusqlite::Result<Append> {
        let tx = self.conn.transaction()?;
        let log = log_of(record);
        let mut added = Vec::new();
        for stub in stubs {
            let held = tx.prepare_cached("SELECT 1 FROM records WHERE id = ?1")?.exists([&stub.id().as_bytes()[..]])?;
            if held || log_name(stub.server(), stub.scope()) != log {
                continue;
            }
            match check_context(&tx, &log, stub.server(), stub.deps(), stub.auth_deps())? {
                Ok(missing) if missing.is_empty() => {}
                _ => return Ok(Append::StubPending),
            }
            insert(&tx, &stub.id(), &log, &stub.encode(), true, stub.deps())?;
            added.push(stub);
        }
        let result = append_in(&tx, record, auth)?;
        if result != Append::Added {
            return Ok(result);
        }
        for stub in added {
            let view = ChannelLog { conn: &tx, log: log.clone(), server: stub.server() };
            if gate.check(stub, &view) != StubVerdict::Accept {
                return Ok(Append::StubPending);
            }
        }
        tx.commit()?;
        Ok(Append::Added)
    }

    /// Replaces a held full record by its stub, if it can be pruned and
    /// `gate` accepts the stub (a held delete or newer edit replaced it).
    /// True if pruned. Only after this do we serve the stub.
    pub fn prune(&mut self, id: &RecordId, gate: &dyn StubGate) -> rusqlite::Result<bool> {
        let tx = self.conn.transaction()?;
        let row: Option<(Vec<u8>, bool, String)> = tx
            .prepare_cached("SELECT data, stub, log FROM records WHERE id = ?1")?
            .query_row([&id.as_bytes()[..]], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((bytes, false, log)) = row else { return Ok(false) };
        let Some(stub) = Record::decode(&bytes).ok().and_then(|r| r.prune()) else { return Ok(false) };
        let view = ChannelLog { conn: &tx, log, server: stub.server() };
        if gate.check(&stub, &view) != StubVerdict::Accept {
            return Ok(false);
        }
        tx.prepare_cached("UPDATE records SET data = ?1, stub = 1 WHERE id = ?2")?
            .execute(params![stub.encode(), &id.as_bytes()[..]])?;
        tx.commit()?;
        Ok(true)
    }

    /// Read access to a server's stored control log (for a [`crate::ReadGate`]).
    pub fn control_log(&self, server: crate::ServerId) -> ControlLog<'_> {
        ControlLog { conn: &self.conn, log: log_name(server, Scope::Control) }
    }

    /// Records of the log that nothing else in the log depends on.
    pub fn heads(&self, log: &str) -> rusqlite::Result<Vec<RecordId>> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM heads WHERE log = ?1 ORDER BY id")?;
        let rows = stmt.query_map([log], |r| r.get::<_, [u8; 32]>(0).map(id_of))?;
        rows.collect()
    }

    /// What to tell a peer we have: our heads plus a few older records at
    /// exponentially growing distances (like git's negotiation). Even when the
    /// peer doesn't know our newest records, it most likely knows one of
    /// these, so it won't resend the shared history. At most [`MAX_HAVE`] ids.
    pub fn have_markers(&self, log: &str) -> rusqlite::Result<Vec<RecordId>> {
        let mut have = self.heads(log)?;
        have.truncate(MAX_HAVE_HEADS);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM records WHERE log = ?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2")?;
        let mut offset = 1i64;
        while have.len() < MAX_HAVE {
            let id: Option<[u8; 32]> = stmt.query_row(params![log, offset], |r| r.get(0)).optional()?;
            let Some(id) = id else { break };
            have.push(id_of(id));
            offset = offset.saturating_mul(2);
        }
        Ok(have)
    }

    /// Records of the log that a peer which has `have` (and all their
    /// ancestors) is missing, in an order they can be appended in. Ids we
    /// don't know are ignored, and so is everything past the first
    /// [`MAX_HAVE`] ids. Returns at most `max_records` records and about
    /// `max_bytes` of data (always at least one if any is missing), and
    /// whether more remain.
    pub fn missing_since(
        &self,
        log: &str,
        have: &[RecordId],
        max_records: usize,
        max_bytes: usize,
    ) -> rusqlite::Result<(Vec<Stored>, bool)> {
        // `temp.have` belongs to this connection only, and each connection is
        // used by one thread at a time, so reusing it between calls is safe;
        // it is emptied first in case an earlier call failed half way.
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS have (id BLOB PRIMARY KEY) WITHOUT ROWID; DELETE FROM temp.have;",
        )?;
        {
            let mut insert = self.conn.prepare_cached("INSERT OR IGNORE INTO temp.have (id) VALUES (?1)")?;
            for id in have.iter().take(MAX_HAVE) {
                insert.execute([&id.as_bytes()[..]])?;
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE known(id) AS (
                 SELECT r.id FROM records r JOIN temp.have h ON h.id = r.id
                 UNION
                 SELECT d.parent FROM deps d JOIN known k ON d.child = k.id
             )
             SELECT id, data, stub FROM records
             WHERE log = ?1 AND id NOT IN known
             ORDER BY seq",
        )?;
        let mut rows = stmt.query([log])?;
        let mut out = Vec::new();
        let mut bytes = 0;
        let mut more = false;
        // Rows are read lazily, so a full page never loads the whole log.
        while let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(1)?;
            if out.len() >= max_records || (bytes + data.len() > max_bytes && !out.is_empty()) {
                more = true;
                break;
            }
            bytes += data.len();
            out.push(Stored { id: id_of(row.get(0)?), bytes: data, stub: row.get(2)? });
        }
        Ok((out, more))
    }

    pub fn count(&self, log: &str) -> rusqlite::Result<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM records WHERE log = ?1", [log], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
    }

    /// Hash of the set of record ids in the log: equal on two peers exactly
    /// when they hold the same records.
    pub fn state_hash(&self, log: &str) -> rusqlite::Result<String> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM records WHERE log = ?1 ORDER BY id")?;
        let mut rows = stmt.query([log])?;
        let mut hasher = blake3::Hasher::new();
        while let Some(row) = rows.next()? {
            hasher.update(&row.get::<_, Vec<u8>>(0)?);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
}

fn check_in(conn: &Connection, record: &Record) -> rusqlite::Result<Result<Vec<RecordId>, IngestError>> {
    check_context(conn, &log_of(record), record.server(), record.deps(), record.auth_deps())
}

/// The context checks of [`check_in`], for a record or a stub (a dep held
/// as a stub counts as present).
fn check_context(
    conn: &Connection,
    log: &str,
    server: ServerId,
    deps: &[RecordId],
    auth_deps: &[RecordId],
) -> rusqlite::Result<Result<Vec<RecordId>, IngestError>> {
    let mut stmt = conn.prepare_cached("SELECT log FROM records WHERE id = ?1")?;
    let mut log_of_id =
        |id: &RecordId| -> rusqlite::Result<Option<String>> { stmt.query_row([&id.as_bytes()[..]], |r| r.get(0)).optional() };
    // A record id covers the record's server and scope, so it belongs to
    // exactly one log: a dep found elsewhere is a definite cross-log dep.
    let mut missing = Vec::new();
    for dep in deps {
        match log_of_id(dep)? {
            Some(dep_log) if dep_log == log => {}
            Some(_) => return Ok(Err(IngestError::DepInOtherLog)),
            None => missing.push(*dep),
        }
    }
    // Channel records name the control-log heads their author relied on for
    // permission; those must be there before anyone can judge the record.
    let control = log_name(server, Scope::Control);
    for dep in auth_deps {
        match log_of_id(dep)? {
            Some(dep_log) if dep_log == control => {}
            Some(_) => return Ok(Err(IngestError::AuthDepNotInControlLog)),
            None => missing.push(*dep),
        }
    }
    Ok(Ok(missing))
}

fn append_in(tx: &rusqlite::Transaction, record: &Record, auth: &dyn Authorizer) -> rusqlite::Result<Append> {
    let id = record.id();
    {
        let mut stmt = tx.prepare_cached("SELECT 1 FROM records WHERE id = ?1")?;
        if stmt.exists([&id.as_bytes()[..]])? {
            return Ok(Append::Known);
        }
    }
    match check_in(tx, record)? {
        Err(err) => return Ok(Append::Invalid(err)),
        Ok(missing) if !missing.is_empty() => return Ok(Append::MissingDeps(missing)),
        Ok(_) => {}
    }
    // Control records are never refused here: authority is a function of
    // the whole set of well-formed control records, and whether one counts
    // can change as concurrent ones arrive. Storing them all keeps every
    // peer's set the same whatever the arrival order; authority decides what
    // they mean. Channel records are judged now, with their auth_deps stored.
    if record.scope() != Scope::Control {
        let control = ControlLog { conn: tx, log: log_name(record.server(), Scope::Control) };
        if auth.check(record, &control) == Verdict::Reject {
            return Ok(Append::Unauthorized);
        }
    }
    insert(tx, &id, &log_of(record), &record.encode(), false, record.deps())?;
    Ok(Append::Added)
}

fn append_stub_in(tx: &rusqlite::Transaction, stub: &Stub, gate: &dyn StubGate) -> rusqlite::Result<Append> {
    let id = stub.id();
    if tx.prepare_cached("SELECT 1 FROM records WHERE id = ?1")?.exists([&id.as_bytes()[..]])? {
        return Ok(Append::Known);
    }
    let log = log_name(stub.server(), stub.scope());
    match check_context(tx, &log, stub.server(), stub.deps(), stub.auth_deps())? {
        Err(err) => return Ok(Append::Invalid(err)),
        Ok(missing) if !missing.is_empty() => return Ok(Append::MissingDeps(missing)),
        Ok(_) => {}
    }
    let view = ChannelLog { conn: tx, log: log.clone(), server: stub.server() };
    match gate.check(stub, &view) {
        StubVerdict::Accept => {}
        StubVerdict::Pending => return Ok(Append::StubPending),
        StubVerdict::Reject => return Ok(Append::Unauthorized),
    }
    insert(tx, &id, &log, &stub.encode(), true, stub.deps())?;
    Ok(Append::Added)
}

/// Adds a row, its deps edges, and updates the log's heads.
fn insert(tx: &rusqlite::Transaction, id: &RecordId, log: &str, data: &[u8], stub: bool, deps: &[RecordId]) -> rusqlite::Result<()> {
    tx.prepare_cached("INSERT INTO records (id, log, data, stub) VALUES (?1, ?2, ?3, ?4)")?
        .execute(params![&id.as_bytes()[..], log, data, stub])?;
    let mut add_dep = tx.prepare_cached("INSERT OR IGNORE INTO deps (child, parent) VALUES (?1, ?2)")?;
    let mut drop_head = tx.prepare_cached("DELETE FROM heads WHERE log = ?1 AND id = ?2")?;
    for dep in deps {
        add_dep.execute([&id.as_bytes()[..], &dep.as_bytes()[..]])?;
        drop_head.execute(params![log, &dep.as_bytes()[..]])?;
    }
    tx.prepare_cached("INSERT INTO heads (log, id) VALUES (?1, ?2)")?.execute(params![log, &id.as_bytes()[..]])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{channel, control_by, genesis, message, message_chain, message_with_auth, naming, post, server};
    use crate::AcceptAll;

    fn chan() -> String {
        log_name(server(), Scope::Channel(channel()))
    }

    #[test]
    fn rejects_record_with_missing_deps() {
        let mut store = Store::in_memory().unwrap();
        let a = message(0, &[], 0);
        let b = message(1, &[a.id()], 0);
        assert_eq!(store.append(&b, &AcceptAll).unwrap(), Append::MissingDeps(vec![a.id()]));
        assert_eq!(store.append(&a, &AcceptAll).unwrap(), Append::Added);
        assert_eq!(store.append(&a, &AcceptAll).unwrap(), Append::Known);
        assert_eq!(store.append(&b, &AcceptAll).unwrap(), Append::Added);
        assert_eq!(store.heads(&chan()).unwrap(), vec![b.id()]);
        // What comes back is the canonical encoding, and decodes to the same record.
        assert_eq!(store.get(&b.id()).unwrap().unwrap(), b.encode());
        assert_eq!(store.get_record(&b.id()).unwrap().unwrap(), b);
    }

    #[test]
    fn records_are_filed_under_the_log_they_signed() {
        let mut store = Store::in_memory().unwrap();
        let g = genesis();
        store.append(&g, &AcceptAll).unwrap();
        store.append(&post(1, &[g.id()]), &AcceptAll).unwrap();
        store.append(&message(0, &[], 0), &AcceptAll).unwrap();
        assert_eq!(store.count(&log_name(server(), Scope::Control)).unwrap(), 2);
        assert_eq!(store.count(&chan()).unwrap(), 1);
    }

    #[test]
    fn deps_must_be_in_the_same_log() {
        let mut store = Store::in_memory().unwrap();
        let g = genesis();
        store.append(&g, &AcceptAll).unwrap();
        // A channel message whose dep is a control record.
        let cross = message(0, &[g.id()], 0);
        assert_eq!(store.append(&cross, &AcceptAll).unwrap(), Append::Invalid(IngestError::DepInOtherLog));
        assert_eq!(store.count(&chan()).unwrap(), 0);
    }

    #[test]
    fn auth_deps_must_be_present_and_in_the_control_log() {
        let mut store = Store::in_memory().unwrap();
        let g = genesis();
        let other = message(0, &[], 0);
        store.append(&other, &AcceptAll).unwrap();

        // Missing control head: wait for it, like a missing dep.
        let m = message_with_auth(1, &[], &[g.id()], 0);
        assert_eq!(store.check(&m).unwrap(), Ok(vec![g.id()]));
        assert_eq!(store.append(&m, &AcceptAll).unwrap(), Append::MissingDeps(vec![g.id()]));
        store.append(&g, &AcceptAll).unwrap();
        assert_eq!(store.append(&m, &AcceptAll).unwrap(), Append::Added);

        // An auth_dep that is a channel record, not a control one.
        let bad = message_with_auth(2, &[], &[other.id()], 0);
        assert_eq!(store.append(&bad, &AcceptAll).unwrap(), Append::Invalid(IngestError::AuthDepNotInControlLog));
    }

    /// Refuses every record of one author, as authority would for a kicked
    /// member or a non-member key.
    /// A monotone storage gate, as the `Authorizer` contract requires: the
    /// author is the owner (wrote the genesis), or a stored `Grant` names
    /// them. Kicks and revokes are deliberately ignored.
    struct OwnerOrGranted;

    impl Authorizer for OwnerOrGranted {
        fn check(&self, record: &Record, control: &ControlLog<'_>) -> Verdict {
            let author = record.author();
            let allowed = control.records().unwrap_or_default().iter().any(|c| match c.kind() {
                records::Kind::Genesis => c.author() == author,
                records::Kind::Grant => c.body().get(..32) == Some(&author.as_bytes()[..]),
                _ => false,
            });
            if allowed { Verdict::Accept } else { Verdict::Reject }
        }
    }

    #[test]
    fn the_storage_gate_only_ever_opens_as_control_records_arrive() {
        use records::Kind;
        let mut store = Store::in_memory().unwrap();
        let g = genesis();
        let m = message(0, &[], 0); // by test key 1, not the owner
        assert_eq!(store.append(&g, &OwnerOrGranted).unwrap(), Append::Added);
        assert_eq!(store.append(&m, &OwnerOrGranted).unwrap(), Append::Unauthorized);
        assert_eq!(store.append_all([&m], &OwnerOrGranted).unwrap(), vec![Append::Unauthorized]);
        assert!(!store.has(&m.id()).unwrap());
        // The authorizer is only asked once the context is there: a record
        // with a missing dep is reported as missing, not judged.
        let child = message(1, &[m.id()], 0);
        assert_eq!(store.append(&child, &OwnerOrGranted).unwrap(), Append::MissingDeps(vec![m.id()]));

        // A grant for key 1 arrives: the same message is now stored.
        let grant = naming(Kind::Grant, 1, &[g.id()], 1);
        store.append(&grant, &OwnerOrGranted).unwrap();
        assert_eq!(store.append(&m, &OwnerOrGranted).unwrap(), Append::Added);
        assert_eq!(store.append(&child, &OwnerOrGranted).unwrap(), Append::Added);

        // A kick arriving later changes nothing in the store: key 1's stored
        // messages stay, and so do honest children of them. Whether they're
        // shown is decided at display time.
        let kick = naming(Kind::Kick, 2, &[grant.id()], 1);
        store.append(&kick, &OwnerOrGranted).unwrap();
        assert!(store.has(&m.id()).unwrap() && store.has(&child.id()).unwrap());
        let grandchild = message(2, &[child.id()], 0);
        assert_eq!(store.append(&grandchild, &OwnerOrGranted).unwrap(), Append::Added);
    }

    /// Records what `ControlLog::count` said each time it was asked.
    struct Counting(std::sync::Mutex<Vec<u64>>);

    impl Authorizer for Counting {
        fn check(&self, _: &Record, control: &ControlLog<'_>) -> Verdict {
            let n = control.count().unwrap();
            assert_eq!(n as usize, control.records().unwrap().len());
            self.0.lock().unwrap().push(n);
            Verdict::Accept
        }
    }

    #[test]
    fn control_log_count_is_the_number_of_stored_control_records() {
        use records::Kind;
        let mut store = Store::in_memory().unwrap();
        let gate = Counting(Default::default());
        let g = genesis();
        store.append(&g, &gate).unwrap();
        store.append(&message(0, &[], 0), &gate).unwrap();
        let grant = naming(Kind::Grant, 1, &[g.id()], 1);
        store.append(&grant, &gate).unwrap();
        store.append(&message(1, &[], 0), &gate).unwrap();
        // Only channel records reach the gate; channel records aren't counted.
        assert_eq!(*gate.0.lock().unwrap(), vec![1, 2]);
    }

    /// Refuses everything, like an authority that hasn't seen a grant yet.
    struct RefuseAll;

    impl Authorizer for RefuseAll {
        fn check(&self, _: &Record, _: &ControlLog<'_>) -> Verdict {
            Verdict::Reject
        }
    }

    #[test]
    fn control_records_are_stored_whatever_the_order() {
        use records::Kind;
        let g = genesis();
        // A concurrent grant and a post by the grantee, both on the genesis,
        // then a post built on the grantee's post.
        let grant = control_by(Kind::Grant, 1, &[g.id()], 0);
        let early = control_by(Kind::Post, 1, &[g.id()], 1);
        let later = control_by(Kind::Post, 2, &[early.id()], 1);
        let control = log_name(server(), Scope::Control);

        let mut a = Store::in_memory().unwrap();
        let mut b = Store::in_memory().unwrap();
        // a sees the grant first; b sees the post before the grant. Even an
        // authorizer that refuses everything doesn't change what is stored.
        for r in [&g, &grant, &early, &later] {
            assert_eq!(a.append(r, &RefuseAll).unwrap(), Append::Added);
        }
        for r in [&g, &early, &later, &grant] {
            assert_eq!(b.append(r, &RefuseAll).unwrap(), Append::Added);
        }
        assert_eq!(a.count(&control).unwrap(), 4);
        assert_eq!(a.state_hash(&control).unwrap(), b.state_hash(&control).unwrap());
        assert_eq!(a.heads(&control).unwrap(), b.heads(&control).unwrap());
        // Nothing left for either to pull from the other.
        assert!(a.missing_since(&control, &b.have_markers(&control).unwrap(), 100, usize::MAX).unwrap().0.is_empty());
        assert!(b.missing_since(&control, &a.have_markers(&control).unwrap(), 100, usize::MAX).unwrap().0.is_empty());
    }

    #[test]
    fn a_refused_record_listed_as_have_is_not_sent_again() {
        // The responder holds a → r; the requester refused r (its deps were
        // all present, so claiming it hides nothing the requester needs).
        let mut responder = Store::in_memory().unwrap();
        let chain = message_chain(3, 0);
        responder.append_all(&chain, &AcceptAll).unwrap();
        let (a, r, after) = (&chain[0], &chain[1], &chain[2]);
        let (page, _) = responder.missing_since(&chan(), &[a.id()], 100, usize::MAX).unwrap();
        assert_eq!(page.len(), 2, "without the marker, r and its child are re-sent");
        let (page, _) = responder.missing_since(&chan(), &[a.id(), r.id()], 100, usize::MAX).unwrap();
        assert_eq!(page.iter().map(|s| s.id).collect::<Vec<_>>(), vec![after.id()]);
    }

    #[test]
    fn missing_since_sends_only_the_gap() {
        let mut store = Store::in_memory().unwrap();
        let base = message_chain(100, 0);
        store.append_all(&base, &AcceptAll).unwrap();
        let tip = base[99].id();
        // Two concurrent branches; the peer has base + branch x, we have both.
        let x: Vec<Record> = (0..10).map(|i| message(1000 + i, &[tip], 0)).collect();
        let y: Vec<Record> = (0..7).map(|i| message(2000 + i, &[tip], 0)).collect();
        store.append_all(x.iter().chain(&y), &AcceptAll).unwrap();
        let mut have: Vec<RecordId> = x.iter().map(Record::id).collect();
        have.sort();
        let (missing, more) = store.missing_since(&chan(), &have, 1000, usize::MAX).unwrap();
        assert!(!more);
        assert_eq!(missing.iter().map(|s| s.id).collect::<Vec<_>>(), y.iter().map(Record::id).collect::<Vec<_>>());

        // A peer that only knows an old marker gets everything after it.
        let (missing, _) = store.missing_since(&chan(), &[base[49].id()], 1000, usize::MAX).unwrap();
        assert_eq!(missing.len(), 50 + 10 + 7);

        // Paging, and every served record decodes back to itself.
        let (page, more) = store.missing_since(&chan(), &[], 30, usize::MAX).unwrap();
        assert!(more);
        assert_eq!(page.len(), 30);
        for s in &page {
            assert_eq!(Record::decode(&s.bytes).unwrap().id(), s.id);
        }
    }

    #[test]
    fn pages_are_bounded_by_bytes() {
        let mut store = Store::in_memory().unwrap();
        let chain = message_chain(20, 10_000);
        store.append_all(&chain, &AcceptAll).unwrap();
        let size = chain[1].encode().len();
        let (page, more) = store.missing_since(&chan(), &[], 1000, 5 * size + size / 2).unwrap();
        assert!(more);
        assert_eq!(page.len(), 5);
        // A single record larger than the budget still goes out, alone.
        let (page, more) = store.missing_since(&chan(), &[], 1000, 1).unwrap();
        assert!(more);
        assert_eq!(page.len(), 1);
    }

    #[test]
    fn have_list_is_capped() {
        let mut store = Store::in_memory().unwrap();
        let chain = message_chain(10, 0);
        store.append_all(&chain, &AcceptAll).unwrap();
        // Only the first MAX_HAVE ids count: the real marker, placed after a
        // flood of unknown ids, is ignored and the whole log is sent.
        let mut have = vec![RecordId::from_bytes([0xee; 32]); MAX_HAVE];
        have.push(chain[9].id());
        let (missing, _) = store.missing_since(&chan(), &have, 1000, usize::MAX).unwrap();
        assert_eq!(missing.len(), 10);
        // Many heads still produce at most MAX_HAVE markers.
        for i in 0..100 {
            store.append(&message(100 + i, &[chain[9].id()], 0), &AcceptAll).unwrap();
        }
        assert_eq!(store.heads(&chan()).unwrap().len(), 100);
        assert!(store.have_markers(&chan()).unwrap().len() <= MAX_HAVE);
    }

    #[test]
    fn corrupt_id_in_database_is_an_error_not_a_panic() {
        let mut store = Store::in_memory().unwrap();
        store.append(&message(0, &[], 0), &AcceptAll).unwrap();
        store.conn.execute("UPDATE heads SET id = x'0102'", []).unwrap();
        assert!(store.heads(&chan()).is_err());
        assert!(store.have_markers(&chan()).is_err());
        // Corrupt record bytes are not handed out as a record.
        store.conn.execute("UPDATE records SET data = x'00'", []).unwrap();
        assert_eq!(store.get_record(&message(0, &[], 0).id()).unwrap(), None);
    }

    #[test]
    fn read_only_connection_serves_while_the_writer_writes() {
        let dir = std::env::temp_dir().join(format!("sync-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        let _ = std::fs::remove_file(&path);
        let mut writer = Store::open(&path).unwrap();
        let reader = Store::open_read_only(&path).unwrap();
        let chain = message_chain(5, 0);
        writer.append_all(&chain[..3], &AcceptAll).unwrap();
        assert_eq!(reader.missing_since(&chan(), &[], 100, usize::MAX).unwrap().0.len(), 3);
        writer.append_all(&chain[3..], &AcceptAll).unwrap();
        assert_eq!(reader.missing_since(&chan(), &[chain[2].id()], 100, usize::MAX).unwrap().0.len(), 2);
        // It cannot write.
        assert!(reader.conn.execute("DELETE FROM records", []).is_err());
        drop((writer, reader));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_stores_converge_through_markers() {
        let mut a = Store::in_memory().unwrap();
        let mut b = Store::in_memory().unwrap();
        let shared = message_chain(500, 0);
        a.append_all(&shared, &AcceptAll).unwrap();
        b.append_all(&shared, &AcceptAll).unwrap();
        let tip = shared[499].id();
        let mut prev = tip;
        for i in 0..300 {
            let r = message(10_000 + i, &[prev], 0);
            prev = r.id();
            a.append(&r, &AcceptAll).unwrap();
        }
        let mut prev = tip;
        for i in 0..200 {
            let r = message(20_000 + i, &[prev], 1);
            prev = r.id();
            b.append(&r, &AcceptAll).unwrap();
        }

        // b pulls from a, sending markers; a doesn't know b's head but knows
        // an older marker, so it must not resend the 500 shared records.
        let (gap, _) = a.missing_since(&chan(), &b.have_markers(&chan()).unwrap(), 10_000, usize::MAX).unwrap();
        assert!(gap.len() < 300 + 500 / 2, "sent {}", gap.len());
        let decoded: Vec<Record> = gap.iter().map(|s| Record::decode(&s.bytes).unwrap()).collect();
        b.append_all(&decoded, &AcceptAll).unwrap();
        let (gap, _) = b.missing_since(&chan(), &a.have_markers(&chan()).unwrap(), 10_000, usize::MAX).unwrap();
        let decoded: Vec<Record> = gap.iter().map(|s| Record::decode(&s.bytes).unwrap()).collect();
        a.append_all(&decoded, &AcceptAll).unwrap();

        assert_eq!(a.count(&chan()).unwrap(), 1000);
        assert_eq!(a.state_hash(&chan()).unwrap(), b.state_hash(&chan()).unwrap());
        assert_eq!(a.heads(&chan()).unwrap(), b.heads(&chan()).unwrap());
    }

    use crate::testutil::{delete, DeletedOnly};

    /// Rule 2: we prune (and so serve a stub) only once a held delete
    /// replaced the record.
    #[test]
    fn a_record_is_pruned_only_once_its_delete_is_held() {
        let mut store = Store::in_memory().unwrap();
        let m = message(0, &[], 10);
        store.append(&m, &AcceptAll).unwrap();
        assert!(!store.prune(&m.id(), &DeletedOnly).unwrap());
        assert_eq!(store.entry(&m.id()).unwrap(), Some(Held::Full(m.clone())));

        let d = delete(1, &m);
        assert_eq!(store.append(&d, &AcceptAll).unwrap(), Append::Added);
        assert!(store.prune(&m.id(), &DeletedOnly).unwrap());
        let stub = m.prune().unwrap();
        assert_eq!(store.entry(&m.id()).unwrap(), Some(Held::Stub(stub.clone())));
        assert_eq!(store.get_record(&m.id()).unwrap(), None);
        // Served as a stub, tagged, in order; the delete still follows it.
        let (page, _) = store.missing_since(&chan(), &[], 100, 1 << 20).unwrap();
        assert_eq!(page[0], Stored { id: m.id(), bytes: stub.encode(), stub: true });
        assert!(!page[1].stub);
        // A delete can't be pruned, nor a stub pruned again.
        assert!(!store.prune(&d.id(), &DeletedOnly).unwrap());
        assert!(!store.prune(&m.id(), &DeletedOnly).unwrap());
        // The id stays the same, so peers' state hashes still match.
        let mut full = Store::in_memory().unwrap();
        full.append_all([&m, &d], &AcceptAll).unwrap();
        assert_eq!(store.state_hash(&chan()).unwrap(), full.state_hash(&chan()).unwrap());
    }

    /// Rule 3: a peer's stub never replaces a full record we hold.
    #[test]
    fn a_peer_stub_never_replaces_a_held_record() {
        let mut store = Store::in_memory().unwrap();
        let m = message(0, &[], 10);
        let d = delete(1, &m);
        store.append_all([&m, &d], &AcceptAll).unwrap();
        // Even though the gate would accept it (the delete is held).
        assert_eq!(store.append_stub(&m.prune().unwrap(), &DeletedOnly).unwrap(), Append::Known);
        assert_eq!(store.entry(&m.id()).unwrap(), Some(Held::Full(m)));
    }

    /// A stub nothing replaced is pending, not stored; when everyone else
    /// pruned, the stub and the delete arrive in that order and are stored
    /// together.
    #[test]
    fn a_stub_that_arrived_before_its_delete_ends_up_accepted() {
        let m = message(0, &[], 10);
        let d = delete(1, &m);
        let stub = m.prune().unwrap();
        let mut store = Store::in_memory().unwrap();
        assert_eq!(store.append_stub(&stub, &DeletedOnly).unwrap(), Append::StubPending);
        assert!(!store.has(&m.id()).unwrap());
        // The delete alone waits for its target.
        assert_eq!(store.append(&d, &AcceptAll).unwrap(), Append::MissingDeps(vec![m.id()]));
        // With the pending stub: both go in.
        assert_eq!(store.append_with_stubs(&d, std::slice::from_ref(&stub), &AcceptAll, &DeletedOnly).unwrap(), Append::Added);
        assert_eq!(store.entry(&m.id()).unwrap(), Some(Held::Stub(stub)));
        assert!(store.has(&d.id()).unwrap());
        assert_eq!(store.heads(&chan()).unwrap(), vec![d.id()]);
    }

    /// A record that depends on a stub but doesn't replace it: nothing is
    /// stored (the stub still needs its delete, or the full record).
    #[test]
    fn a_stub_is_not_taken_in_with_a_record_that_does_not_replace_it() {
        let m = message(0, &[], 10);
        let reply = message(1, &[m.id()], 0);
        let mut store = Store::in_memory().unwrap();
        let result = store.append_with_stubs(&reply, &[m.prune().unwrap()], &AcceptAll, &DeletedOnly).unwrap();
        assert_eq!(result, Append::StubPending);
        assert!(!store.has(&m.id()).unwrap() && !store.has(&reply.id()).unwrap());
    }
}
