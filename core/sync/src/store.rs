//! SQLite store of opaque, content-addressed blobs grouped into logs.
//!
//! A blob is only accepted once all of its deps are stored, so the local
//! insertion order (`seq`) is always a topological order of the DAG, and
//! "I have X" always implies "I have every ancestor of X". A blob is also only
//! accepted if its id matches its content ([`Blob::verify`]).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::{Blob, BlobError, Id};

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
    /// Not stored: these deps are not in the store yet.
    MissingDeps(Vec<Id>),
    /// Not stored: the id doesn't match the content, or a limit is exceeded.
    Invalid(BlobError),
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS blobs (
                 seq  INTEGER PRIMARY KEY AUTOINCREMENT,
                 id   BLOB NOT NULL UNIQUE,
                 log  TEXT NOT NULL,
                 data BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS blobs_log ON blobs (log, seq);
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
        Ok(Self { conn })
    }

    pub fn has(&self, id: &Id) -> rusqlite::Result<bool> {
        self.conn
            .query_row("SELECT 1 FROM blobs WHERE id = ?1", [&id[..]], |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
    }

    pub fn append(&mut self, log: &str, blob: &Blob) -> rusqlite::Result<Append> {
        let tx = self.conn.transaction()?;
        let result = append_in(&tx, log, blob)?;
        tx.commit()?;
        Ok(result)
    }

    /// Appends many blobs in one transaction (much faster than one by one).
    /// Returns the ids added and the blobs skipped (missing deps or invalid).
    pub fn append_all<'a>(
        &mut self,
        log: &str,
        blobs: impl IntoIterator<Item = &'a Blob>,
    ) -> rusqlite::Result<(Vec<Id>, Vec<&'a Blob>)> {
        let tx = self.conn.transaction()?;
        let mut added = Vec::new();
        let mut skipped = Vec::new();
        for blob in blobs {
            match append_in(&tx, log, blob)? {
                Append::Added => added.push(blob.id),
                Append::Known => {}
                Append::MissingDeps(_) | Append::Invalid(_) => skipped.push(blob),
            }
        }
        tx.commit()?;
        Ok((added, skipped))
    }

    pub fn get(&self, id: &Id) -> rusqlite::Result<Option<Blob>> {
        let data: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT data FROM blobs WHERE id = ?1", [&id[..]], |r| r.get(0))
            .optional()?;
        let Some(data) = data else { return Ok(None) };
        Ok(Some(Blob { id: *id, deps: self.deps_of(id)?, data }))
    }

    fn deps_of(&self, id: &Id) -> rusqlite::Result<Vec<Id>> {
        let mut stmt = self.conn.prepare_cached("SELECT parent FROM deps WHERE child = ?1 ORDER BY parent")?;
        let rows = stmt.query_map([&id[..]], |r| r.get::<_, Id>(0))?;
        rows.collect()
    }

    /// Blobs of the log that nothing else in the log depends on.
    pub fn heads(&self, log: &str) -> rusqlite::Result<Vec<Id>> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM heads WHERE log = ?1 ORDER BY id")?;
        let rows = stmt.query_map([log], |r| r.get::<_, Id>(0))?;
        rows.collect()
    }

    /// What to tell a peer we have: our heads plus a few older blobs at
    /// exponentially growing distances (like git's negotiation). Even when the
    /// peer doesn't know our newest blobs, it most likely knows one of these,
    /// so it won't resend the shared history. At most [`MAX_HAVE`] ids.
    pub fn have_markers(&self, log: &str) -> rusqlite::Result<Vec<Id>> {
        let mut have = self.heads(log)?;
        have.truncate(MAX_HAVE_HEADS);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM blobs WHERE log = ?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2")?;
        let mut offset = 1i64;
        while have.len() < MAX_HAVE {
            let id: Option<Id> = stmt.query_row(params![log, offset], |r| r.get(0)).optional()?;
            let Some(id) = id else { break };
            have.push(id);
            offset = offset.saturating_mul(2);
        }
        Ok(have)
    }

    /// Blobs of the log that a peer which has `have` (and all their ancestors)
    /// is missing, in an order they can be appended in. Ids we don't know are
    /// ignored, and so is everything past the first [`MAX_HAVE`] ids. Returns
    /// at most `max_blobs` blobs and about `max_bytes` of data (always at
    /// least one blob if any is missing), and whether more remain.
    pub fn missing_since(
        &self,
        log: &str,
        have: &[Id],
        max_blobs: usize,
        max_bytes: usize,
    ) -> rusqlite::Result<(Vec<Blob>, bool)> {
        // `temp.have` belongs to this connection only, and the store is used
        // from one task, so reusing it between calls is safe; it is emptied
        // first in case an earlier call failed half way.
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS have (id BLOB PRIMARY KEY) WITHOUT ROWID; DELETE FROM temp.have;",
        )?;
        {
            let mut insert = self.conn.prepare_cached("INSERT OR IGNORE INTO temp.have (id) VALUES (?1)")?;
            for id in have.iter().take(MAX_HAVE) {
                insert.execute([&id[..]])?;
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE known(id) AS (
                 SELECT b.id FROM blobs b JOIN temp.have h ON h.id = b.id
                 UNION
                 SELECT d.parent FROM deps d JOIN known k ON d.child = k.id
             )
             SELECT id, data, (SELECT COUNT(*) FROM deps WHERE child = blobs.id) FROM blobs
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
            let size = data.len() + 32 * row.get::<_, i64>(2)? as usize;
            if out.len() >= max_blobs || (bytes + size > max_bytes && !out.is_empty()) {
                more = true;
                break;
            }
            bytes += size;
            out.push(Blob { id: row.get(0)?, deps: Vec::new(), data });
        }
        drop(rows);
        for blob in &mut out {
            blob.deps = self.deps_of(&blob.id)?;
        }
        Ok((out, more))
    }

    pub fn count(&self, log: &str) -> rusqlite::Result<u64> {
        self.conn.query_row("SELECT COUNT(*) FROM blobs WHERE log = ?1", [log], |r| r.get::<_, i64>(0)).map(|n| n as u64)
    }

    /// Hash of the set of blob ids in the log: equal on two peers exactly when
    /// they hold the same blobs.
    pub fn state_hash(&self, log: &str) -> rusqlite::Result<String> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM blobs WHERE log = ?1 ORDER BY id")?;
        let mut rows = stmt.query([log])?;
        let mut hasher = blake3::Hasher::new();
        while let Some(row) = rows.next()? {
            hasher.update(&row.get::<_, Vec<u8>>(0)?);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
}

fn append_in(tx: &rusqlite::Transaction, log: &str, blob: &Blob) -> rusqlite::Result<Append> {
    // Checked before the "already have it" shortcut: a forged blob claiming a
    // stored id must never look like a duplicate of it either.
    if let Err(err) = blob.verify(log) {
        return Ok(Append::Invalid(err));
    }
    let exists = |id: &Id| -> rusqlite::Result<bool> {
        let mut stmt = tx.prepare_cached("SELECT 1 FROM blobs WHERE id = ?1")?;
        stmt.exists([&id[..]])
    };
    if exists(&blob.id)? {
        return Ok(Append::Known);
    }
    let mut missing = Vec::new();
    for dep in &blob.deps {
        if !exists(dep)? {
            missing.push(*dep);
        }
    }
    if !missing.is_empty() {
        return Ok(Append::MissingDeps(missing));
    }
    tx.prepare_cached("INSERT INTO blobs (id, log, data) VALUES (?1, ?2, ?3)")?.execute(params![
        &blob.id[..],
        log,
        &blob.data
    ])?;
    let mut add_dep = tx.prepare_cached("INSERT OR IGNORE INTO deps (child, parent) VALUES (?1, ?2)")?;
    let mut drop_head = tx.prepare_cached("DELETE FROM heads WHERE log = ?1 AND id = ?2")?;
    for dep in &blob.deps {
        add_dep.execute([&blob.id[..], &dep[..]])?;
        drop_head.execute(params![log, &dep[..]])?;
    }
    tx.prepare_cached("INSERT INTO heads (log, id) VALUES (?1, ?2)")?.execute(params![log, &blob.id[..]])?;
    Ok(Append::Added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_DATA_BYTES, MAX_DEPS};

    fn chain(store: &mut Store, log: &str, parent: &[Id], n: usize, tag: &str) -> Vec<Id> {
        let mut deps = parent.to_vec();
        let mut ids = Vec::new();
        for i in 0..n {
            let blob = Blob::new(log, deps, format!("{tag}{i}").into_bytes());
            assert_eq!(store.append(log, &blob).unwrap(), Append::Added);
            deps = vec![blob.id];
            ids.push(blob.id);
        }
        ids
    }

    #[test]
    fn rejects_blob_with_missing_deps() {
        let mut store = Store::in_memory().unwrap();
        let a = Blob::new("l", vec![], b"a".to_vec());
        let b = Blob::new("l", vec![a.id], b"b".to_vec());
        assert_eq!(store.append("l", &b).unwrap(), Append::MissingDeps(vec![a.id]));
        assert_eq!(store.append("l", &a).unwrap(), Append::Added);
        assert_eq!(store.append("l", &a).unwrap(), Append::Known);
        assert_eq!(store.append("l", &b).unwrap(), Append::Added);
        assert_eq!(store.heads("l").unwrap(), vec![b.id]);
        assert_eq!(store.get(&b.id).unwrap().unwrap(), b);
    }

    #[test]
    fn forged_blob_cannot_take_a_real_id() {
        let mut store = Store::in_memory().unwrap();
        let real = Blob::new("l", vec![], b"real".to_vec());
        let junk = Blob { id: real.id, deps: vec![], data: b"junk".to_vec() };
        // The attacker's copy arrives first and is refused...
        assert_eq!(store.append("l", &junk).unwrap(), Append::Invalid(BlobError::WrongId));
        let (added, skipped) = store.append_all("l", [&junk]).unwrap();
        assert!(added.is_empty() && skipped.len() == 1);
        // ...so the real one is still accepted and served.
        assert_eq!(store.append("l", &real).unwrap(), Append::Added);
        assert_eq!(store.get(&real.id).unwrap().unwrap().data, b"real");
        // A known id with forged content is refused, not reported as known.
        assert_eq!(store.append("l", &junk).unwrap(), Append::Invalid(BlobError::WrongId));
    }

    #[test]
    fn real_blob_cannot_be_planted_in_another_log() {
        let mut store = Store::in_memory().unwrap();
        let real = Blob::new("channel-a", vec![], b"hello".to_vec());
        assert_eq!(store.append("channel-b", &real).unwrap(), Append::Invalid(BlobError::WrongId));
        assert_eq!(store.append("channel-a", &real).unwrap(), Append::Added);
        assert_eq!(store.count("channel-b").unwrap(), 0);
    }

    #[test]
    fn rejects_oversized_blobs() {
        let mut store = Store::in_memory().unwrap();
        let big = Blob::new("l", vec![], vec![0; MAX_DATA_BYTES + 1]);
        assert!(matches!(store.append("l", &big).unwrap(), Append::Invalid(BlobError::TooLarge(_))));
        let many = Blob::new("l", vec![[1; 32]; MAX_DEPS + 1], vec![]);
        assert!(matches!(store.append("l", &many).unwrap(), Append::Invalid(BlobError::TooManyDeps(_))));
    }

    #[test]
    fn corrupt_id_in_database_is_an_error_not_a_panic() {
        let mut store = Store::in_memory().unwrap();
        let a = Blob::new("l", vec![], b"a".to_vec());
        store.append("l", &a).unwrap();
        store.conn.execute("UPDATE heads SET id = x'0102'", []).unwrap();
        store.conn.execute("UPDATE deps SET parent = x'0102'", []).unwrap();
        assert!(store.heads("l").is_err());
        assert!(store.have_markers("l").is_err());
    }

    #[test]
    fn missing_since_sends_only_the_gap() {
        let mut store = Store::in_memory().unwrap();
        let base = chain(&mut store, "l", &[], 100, "base");
        // Two concurrent branches; the peer has base + branch x, we have both.
        let x = chain(&mut store, "l", &[base[99]], 10, "x");
        let y = chain(&mut store, "l", &[base[99]], 7, "y");
        let (missing, more) = store.missing_since("l", &[x[9]], 1000, usize::MAX).unwrap();
        assert!(!more);
        assert_eq!(missing.iter().map(|b| b.id).collect::<Vec<_>>(), y);

        // A peer that only knows an old marker gets everything after it.
        let (missing, _) = store.missing_since("l", &[base[49]], 1000, usize::MAX).unwrap();
        assert_eq!(missing.len(), 50 + 10 + 7);

        // Paging.
        let (page, more) = store.missing_since("l", &[], 30, usize::MAX).unwrap();
        assert!(more);
        assert_eq!(page.len(), 30);
    }

    #[test]
    fn pages_are_bounded_by_bytes() {
        let mut store = Store::in_memory().unwrap();
        let mut deps = vec![];
        for i in 0..20u8 {
            let blob = Blob::new("l", deps, vec![i; 10_000]);
            store.append("l", &blob).unwrap();
            deps = vec![blob.id];
        }
        // 10,000 bytes of data + one 32-byte dep each: 5 fit in 60,000.
        let (page, more) = store.missing_since("l", &[], 1000, 60_000).unwrap();
        assert!(more);
        assert_eq!(page.len(), 5);
        // A single blob larger than the budget still goes out, alone.
        let (page, more) = store.missing_since("l", &[], 1000, 1).unwrap();
        assert!(more);
        assert_eq!(page.len(), 1);
    }

    #[test]
    fn have_list_is_capped() {
        let mut store = Store::in_memory().unwrap();
        let ids = chain(&mut store, "l", &[], 10, "c");
        // Only the first MAX_HAVE ids count: the real marker, placed after a
        // flood of unknown ids, is ignored and the whole log is sent.
        let mut have = vec![[0xee; 32]; MAX_HAVE];
        have.push(ids[9]);
        let (missing, _) = store.missing_since("l", &have, 1000, usize::MAX).unwrap();
        assert_eq!(missing.len(), 10);
        // Many heads still produce at most MAX_HAVE markers.
        for i in 0..100 {
            store.append("l", &Blob::new("l", vec![ids[9]], vec![i])).unwrap();
        }
        assert_eq!(store.heads("l").unwrap().len(), 100);
        assert!(store.have_markers("l").unwrap().len() <= MAX_HAVE);
    }

    #[test]
    fn two_stores_converge_through_markers() {
        let mut a = Store::in_memory().unwrap();
        let mut b = Store::in_memory().unwrap();
        let shared = chain(&mut a, "l", &[], 500, "s");
        for id in &shared {
            b.append("l", &a.get(id).unwrap().unwrap()).unwrap();
        }
        chain(&mut a, "l", &[shared[499]], 300, "a");
        chain(&mut b, "l", &[shared[499]], 200, "b");

        // b pulls from a, sending markers; a doesn't know b's head but knows
        // an older marker, so it must not resend the 500 shared blobs.
        let (gap, _) = a.missing_since("l", &b.have_markers("l").unwrap(), 10_000, usize::MAX).unwrap();
        assert!(gap.len() < 300 + 500 / 2, "sent {}", gap.len());
        b.append_all("l", &gap).unwrap();
        let (gap, _) = b.missing_since("l", &a.have_markers("l").unwrap(), 10_000, usize::MAX).unwrap();
        a.append_all("l", &gap).unwrap();

        assert_eq!(a.count("l").unwrap(), 1000);
        assert_eq!(a.state_hash("l").unwrap(), b.state_hash("l").unwrap());
        assert_eq!(a.heads("l").unwrap(), b.heads("l").unwrap());
    }
}
