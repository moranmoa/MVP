//! SQLite store of opaque, content-addressed blobs grouped into logs.
//!
//! A blob is only accepted once all of its deps are stored, so the local
//! insertion order (`seq`) is always a topological order of the DAG, and
//! "I have X" always implies "I have every ancestor of X".

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::{Blob, Id};

#[derive(Debug, PartialEq, Eq)]
pub enum Append {
    /// Newly stored.
    Added,
    /// Already had it.
    Known,
    /// Not stored: these deps are not in the store yet.
    MissingDeps(Vec<Id>),
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
    /// Blobs whose deps are missing are skipped and returned.
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
                Append::MissingDeps(_) => skipped.push(blob),
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
        let rows = stmt.query_map([&id[..]], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|r| r.map(|v| to_id(&v))).collect()
    }

    /// Blobs of the log that nothing else in the log depends on.
    pub fn heads(&self, log: &str) -> rusqlite::Result<Vec<Id>> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM heads WHERE log = ?1 ORDER BY id")?;
        let rows = stmt.query_map([log], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|r| r.map(|v| to_id(&v))).collect()
    }

    /// What to tell a peer we have: our heads plus a few older blobs at
    /// exponentially growing distances (like git's negotiation). Even when the
    /// peer doesn't know our newest blobs, it most likely knows one of these,
    /// so it won't resend the shared history.
    pub fn have_markers(&self, log: &str) -> rusqlite::Result<Vec<Id>> {
        let mut have = self.heads(log)?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM blobs WHERE log = ?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2")?;
        let mut offset = 1i64;
        loop {
            let id: Option<Vec<u8>> = stmt.query_row(params![log, offset], |r| r.get(0)).optional()?;
            let Some(id) = id else { break };
            have.push(to_id(&id));
            offset *= 2;
        }
        Ok(have)
    }

    /// Blobs of the log that a peer which has `have` (and all their ancestors)
    /// is missing, in an order they can be appended in. Ids we don't know are
    /// ignored. Returns at most `limit` blobs and whether more remain.
    pub fn missing_since(&self, log: &str, have: &[Id], limit: usize) -> rusqlite::Result<(Vec<Blob>, bool)> {
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS have (id BLOB PRIMARY KEY) WITHOUT ROWID; DELETE FROM temp.have;",
        )?;
        {
            let mut insert = self.conn.prepare_cached("INSERT OR IGNORE INTO temp.have (id) VALUES (?1)")?;
            for id in have {
                insert.execute([&id[..]])?;
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE known(id) AS (
                 SELECT b.id FROM blobs b JOIN temp.have h ON h.id = b.id
                 UNION
                 SELECT d.parent FROM deps d JOIN known k ON d.child = k.id
             )
             SELECT id, data FROM blobs
             WHERE log = ?1 AND id NOT IN known
             ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![log, limit as i64 + 1], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            let id = to_id(&id);
            out.push(Blob { id, deps: Vec::new(), data });
        }
        let more = out.len() > limit;
        out.truncate(limit);
        for blob in &mut out {
            blob.deps = self.deps_of(&blob.id)?;
        }
        Ok((out, more))
    }

    pub fn count(&self, log: &str) -> rusqlite::Result<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM blobs WHERE log = ?1", [log], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
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
    tx.prepare_cached("INSERT INTO blobs (id, log, data) VALUES (?1, ?2, ?3)")?
        .execute(params![&blob.id[..], log, &blob.data])?;
    let mut add_dep = tx.prepare_cached("INSERT OR IGNORE INTO deps (child, parent) VALUES (?1, ?2)")?;
    let mut drop_head = tx.prepare_cached("DELETE FROM heads WHERE log = ?1 AND id = ?2")?;
    for dep in &blob.deps {
        add_dep.execute([&blob.id[..], &dep[..]])?;
        drop_head.execute(params![log, &dep[..]])?;
    }
    tx.prepare_cached("INSERT INTO heads (log, id) VALUES (?1, ?2)")?
        .execute(params![log, &blob.id[..]])?;
    Ok(Append::Added)
}

fn to_id(bytes: &[u8]) -> Id {
    let mut id = [0u8; 32];
    id.copy_from_slice(bytes);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(store: &mut Store, log: &str, parent: &[Id], n: usize, tag: &str) -> Vec<Id> {
        let mut deps = parent.to_vec();
        let mut ids = Vec::new();
        for i in 0..n {
            let blob = Blob::new(deps, format!("{tag}{i}").into_bytes());
            assert_eq!(store.append(log, &blob).unwrap(), Append::Added);
            deps = vec![blob.id];
            ids.push(blob.id);
        }
        ids
    }

    #[test]
    fn rejects_blob_with_missing_deps() {
        let mut store = Store::in_memory().unwrap();
        let a = Blob::new(vec![], b"a".to_vec());
        let b = Blob::new(vec![a.id], b"b".to_vec());
        assert_eq!(store.append("l", &b).unwrap(), Append::MissingDeps(vec![a.id]));
        assert_eq!(store.append("l", &a).unwrap(), Append::Added);
        assert_eq!(store.append("l", &a).unwrap(), Append::Known);
        assert_eq!(store.append("l", &b).unwrap(), Append::Added);
        assert_eq!(store.heads("l").unwrap(), vec![b.id]);
        assert_eq!(store.get(&b.id).unwrap().unwrap(), b);
    }

    #[test]
    fn missing_since_sends_only_the_gap() {
        let mut store = Store::in_memory().unwrap();
        let base = chain(&mut store, "l", &[], 100, "base");
        // Two concurrent branches; the peer has base + branch x, we have both.
        let x = chain(&mut store, "l", &[base[99]], 10, "x");
        let y = chain(&mut store, "l", &[base[99]], 7, "y");
        let (missing, more) = store.missing_since("l", &[x[9]], 1000).unwrap();
        assert!(!more);
        assert_eq!(missing.iter().map(|b| b.id).collect::<Vec<_>>(), y);

        // A peer that only knows an old marker gets everything after it.
        let (missing, _) = store.missing_since("l", &[base[49]], 1000).unwrap();
        assert_eq!(missing.len(), 50 + 10 + 7);

        // Paging.
        let (page, more) = store.missing_since("l", &[], 30).unwrap();
        assert!(more);
        assert_eq!(page.len(), 30);
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
        let (gap, _) = a.missing_since("l", &b.have_markers("l").unwrap(), 10_000).unwrap();
        assert!(gap.len() < 300 + 500 / 2, "sent {}", gap.len());
        b.append_all("l", &gap).unwrap();
        let (gap, _) = b.missing_since("l", &a.have_markers("l").unwrap(), 10_000).unwrap();
        a.append_all("l", &gap).unwrap();

        assert_eq!(a.count("l").unwrap(), 1000);
        assert_eq!(a.state_hash("l").unwrap(), b.state_hash("l").unwrap());
        assert_eq!(a.heads("l").unwrap(), b.heads("l").unwrap());
    }
}
