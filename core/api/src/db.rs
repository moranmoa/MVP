//! Local SQLite store: records, the servers I'm in, settings, and the
//! profile (encrypted with the identity's storage key).

use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};
use records::{PublicKey, Record, RecordId, ServerId, Stub};
use rusqlite::{params, Connection, OptionalExtension};
use zeroize::Zeroizing;

use crate::types::{CoreError, ErrorCode, SendState};

const SCHEMA: &str = "
PRAGMA secure_delete = ON;
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS records (
    id BLOB PRIMARY KEY,
    server BLOB NOT NULL,
    bytes BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS records_server ON records (server);
CREATE TABLE IF NOT EXISTS stubs (
    id BLOB PRIMARY KEY,
    server BLOB NOT NULL,
    bytes BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS stubs_server ON stubs (server);
CREATE TABLE IF NOT EXISTS send_states (id BLOB PRIMARY KEY, state TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS servers (id BLOB PRIMARY KEY, joined_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS blocked (user BLOB PRIMARY KEY);
";

const PROFILE_KEY: &str = "profile";
const PROFILE_AD: &[u8] = b"mvs/api/profile/v1";
const SECRET_AD: &[u8] = b"mvs/api/server-secret/v1";

fn secret_key(server: ServerId) -> String {
    format!("server-secret/{}", hex::encode(server.as_bytes()))
}
const NONCE_LEN: usize = 24;

pub(crate) struct Db {
    conn: Connection,
}

fn internal(_: impl std::fmt::Debug) -> CoreError {
    // Storage errors may name paths; the UI only learns the code.
    CoreError::new(ErrorCode::Internal)
}

impl Db {
    pub fn open(path: Option<&Path>) -> Result<Db, CoreError> {
        let conn = match path {
            Some(path) => Connection::open(path),
            None => Connection::open_in_memory(),
        }
        .map_err(internal)?;
        conn.execute_batch(SCHEMA).map_err(internal)?;
        Ok(Db { conn })
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        self.conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .map_err(internal)
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
        self.conn
            .execute("INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)", params![key, value])
            .map(drop)
            .map_err(internal)
    }

    pub fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.conn.execute("DELETE FROM kv WHERE key = ?1", [key]).map(drop).map_err(internal)
    }

    /// The profile JSON, sealed with XChaCha20-Poly1305 under the storage
    /// key: it holds the birth date, which never leaves the device.
    pub fn save_profile(&self, key: &[u8; 32], json: &[u8]) -> Result<(), CoreError> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let sealed = XChaCha20Poly1305::new(key.into())
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: json, aad: PROFILE_AD })
            .map_err(internal)?;
        self.set(PROFILE_KEY, &[nonce.as_slice(), &sealed].concat())
    }

    pub fn load_profile(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, CoreError> {
        let Some(blob) = self.get(PROFILE_KEY)? else { return Ok(None) };
        if blob.len() < NONCE_LEN {
            return Err(internal("short profile"));
        }
        let (nonce, sealed) = blob.split_at(NONCE_LEN);
        XChaCha20Poly1305::new(key.into())
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: PROFILE_AD })
            .map(Some)
            .map_err(internal)
    }

    /// A server's sync secret (GAP-4), sealed like the profile under the
    /// storage key, bound to the server by the AD. A bearer credential: it
    /// never leaves this table except into an invite link the user copies.
    pub fn save_server_secret(
        &self,
        key: &[u8; 32],
        server: ServerId,
        secret: &[u8; 32],
    ) -> Result<(), CoreError> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let aad = [SECRET_AD, server.as_bytes().as_slice()].concat();
        let sealed = XChaCha20Poly1305::new(key.into())
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: secret, aad: &aad })
            .map_err(internal)?;
        self.set(&secret_key(server), &[nonce.as_slice(), &sealed].concat())
    }

    pub fn load_server_secret(
        &self,
        key: &[u8; 32],
        server: ServerId,
    ) -> Result<Option<Zeroizing<[u8; 32]>>, CoreError> {
        let Some(blob) = self.get(&secret_key(server))? else { return Ok(None) };
        if blob.len() < NONCE_LEN {
            return Err(internal("short secret"));
        }
        let (nonce, sealed) = blob.split_at(NONCE_LEN);
        let aad = [SECRET_AD, server.as_bytes().as_slice()].concat();
        let plain = Zeroizing::new(
            XChaCha20Poly1305::new(key.into())
                .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &aad })
                .map_err(internal)?,
        );
        let secret: [u8; 32] = plain.as_slice().try_into().map_err(|_| internal("secret size"))?;
        Ok(Some(Zeroizing::new(secret)))
    }

    pub fn put_record(&self, record: &Record) -> Result<(), CoreError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO records (id, server, bytes) VALUES (?1, ?2, ?3)",
                params![
                    record.id().as_bytes().as_slice(),
                    record.server().as_bytes().as_slice(),
                    record.encode()
                ],
            )
            .map(drop)
            .map_err(internal)
    }

    /// All of `records` or none, in one transaction.
    pub fn put_records(&self, records: &[Record]) -> Result<(), CoreError> {
        let tx = self.conn.unchecked_transaction().map_err(internal)?;
        for record in records {
            self.put_record(record)?;
        }
        tx.commit().map_err(internal)
    }

    pub fn records(&self, server: ServerId) -> Result<Vec<Record>, CoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT bytes FROM records WHERE server = ?1 ORDER BY rowid")
            .map_err(internal)?;
        let rows = stmt
            .query_map([server.as_bytes().as_slice()], |row| row.get::<_, Vec<u8>>(0))
            .map_err(internal)?;
        let mut out = Vec::new();
        for bytes in rows {
            // Stored records were verified on the way in; re-verify anyway.
            out.push(Record::decode(&bytes.map_err(internal)?).map_err(internal)?);
        }
        Ok(out)
    }

    /// Replaces a stored record by its stub (B6-p), in one transaction. With
    /// `secure_delete` on, the record's bytes are overwritten, not left in
    /// free pages.
    pub fn prune_record(&self, stub: &Stub) -> Result<(), CoreError> {
        let tx = self.conn.unchecked_transaction().map_err(internal)?;
        self.conn
            .execute("DELETE FROM records WHERE id = ?1", [stub.id().as_bytes().as_slice()])
            .map_err(internal)?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO stubs (id, server, bytes) VALUES (?1, ?2, ?3)",
                params![
                    stub.id().as_bytes().as_slice(),
                    stub.server().as_bytes().as_slice(),
                    stub.encode()
                ],
            )
            .map_err(internal)?;
        tx.commit().map_err(internal)
    }

    /// B6: the last known send state of a record written here.
    pub fn send_state(&self, id: RecordId) -> Result<Option<SendState>, CoreError> {
        let state: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM send_states WHERE id = ?1",
                [id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)?;
        Ok(state.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok()))
    }

    pub fn set_send_state(&self, id: RecordId, state: SendState) -> Result<(), CoreError> {
        let text = serde_json::to_value(state).map_err(internal)?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO send_states (id, state) VALUES (?1, ?2)",
                params![id.as_bytes().as_slice(), text.as_str().unwrap_or_default()],
            )
            .map(drop)
            .map_err(internal)
    }
    pub fn stubs(&self, server: ServerId) -> Result<Vec<Stub>, CoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT bytes FROM stubs WHERE server = ?1 ORDER BY rowid")
            .map_err(internal)?;
        let rows = stmt
            .query_map([server.as_bytes().as_slice()], |row| row.get::<_, Vec<u8>>(0))
            .map_err(internal)?;
        let mut out = Vec::new();
        for bytes in rows {
            out.push(Stub::decode(&bytes.map_err(internal)?).map_err(internal)?);
        }
        Ok(out)
    }

    pub fn add_server(&self, server: ServerId, joined_at: u64) -> Result<(), CoreError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO servers (id, joined_at) VALUES (?1, ?2)",
                params![server.as_bytes().as_slice(), joined_at as i64],
            )
            .map(drop)
            .map_err(internal)
    }

    pub fn remove_server(&self, server: ServerId) -> Result<(), CoreError> {
        self.conn
            .execute("DELETE FROM servers WHERE id = ?1", [server.as_bytes().as_slice()])
            .map(drop)
            .map_err(internal)
    }

    /// Servers I'm in, oldest first.
    pub fn servers(&self) -> Result<Vec<ServerId>, CoreError> {
        let mut stmt =
            self.conn.prepare("SELECT id FROM servers ORDER BY joined_at, id").map_err(internal)?;
        let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0)).map_err(internal)?;
        let mut out = Vec::new();
        for id in rows {
            let bytes: [u8; 32] = id.map_err(internal)?.try_into().map_err(internal)?;
            out.push(ServerId::from_bytes(bytes));
        }
        Ok(out)
    }

    pub fn has_server(&self, server: ServerId) -> Result<bool, CoreError> {
        Ok(self.servers()?.contains(&server))
    }

    pub fn is_blocked(&self, user: &PublicKey) -> Result<bool, CoreError> {
        self.conn
            .query_row(
                "SELECT 1 FROM blocked WHERE user = ?1",
                [user.as_bytes().as_slice()],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .map_err(internal)
    }

    /// Flips the block; returns whether the user is now blocked.
    pub fn toggle_block(&self, user: &PublicKey) -> Result<bool, CoreError> {
        let key = user.as_bytes().as_slice();
        if self.is_blocked(user)? {
            self.conn.execute("DELETE FROM blocked WHERE user = ?1", [key]).map_err(internal)?;
            Ok(false)
        } else {
            self.conn.execute("INSERT INTO blocked (user) VALUES (?1)", [key]).map_err(internal)?;
            Ok(true)
        }
    }

    pub fn size_bytes(&self) -> Result<u64, CoreError> {
        let pages: i64 =
            self.conn.query_row("PRAGMA page_count", [], |row| row.get(0)).map_err(internal)?;
        let size: i64 =
            self.conn.query_row("PRAGMA page_size", [], |row| row.get(0)).map_err(internal)?;
        Ok((pages * size) as u64)
    }
}
