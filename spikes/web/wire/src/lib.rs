//! The core/sync wire format, buildable for the browser (wasm32).
//!
//! - [`Blob`]: byte-for-byte the same as `sync::Blob` (id, u16 LE dep count,
//!   deps, data; id = BLAKE3 of dep count, deps and data).
//! - [`SyncRequest`] / [`SyncResponse`]: the CBOR request-response messages
//!   of `/descord-replacement/sync/0.1.0`.
//! - [`MemStore`]: an in-memory log with the same pull semantics as
//!   `sync::Store` (have markers, missing-since, state hash), for the browser.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

pub const SYNC_PROTOCOL: &str = "/descord-replacement/sync/0.1.0";
pub const IDENTIFY_VERSION: &str = "/descord-replacement/0.1.0";
/// Blobs per sync response page (same as core/sync).
pub const PAGE: usize = 2000;

pub type Id = [u8; 32];

pub fn topic(log: &str) -> String {
    format!("log/{log}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub id: Id,
    pub deps: Vec<Id>,
    pub data: Vec<u8>,
}

impl Blob {
    pub fn new(deps: Vec<Id>, data: Vec<u8>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(deps.len() as u32).to_le_bytes());
        for dep in &deps {
            hasher.update(dep);
        }
        hasher.update(&data);
        Self { id: *hasher.finalize().as_bytes(), deps, data }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 2 + 32 * self.deps.len() + self.data.len());
        out.extend_from_slice(&self.id);
        out.extend_from_slice(&(self.deps.len() as u16).to_le_bytes());
        for dep in &self.deps {
            out.extend_from_slice(dep);
        }
        out.extend_from_slice(&self.data);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let id: Id = bytes.get(..32)?.try_into().ok()?;
        let n = u16::from_le_bytes(bytes.get(32..34)?.try_into().ok()?) as usize;
        let deps_end = 34 + 32 * n;
        let deps =
            bytes.get(34..deps_end)?.chunks_exact(32).map(|c| c.try_into().unwrap()).collect();
        Some(Self { id, deps, data: bytes[deps_end..].to_vec() })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncRequest {
    pub log: String,
    pub have: Vec<ByteBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub blobs: Vec<ByteBuf>,
    pub more: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Append {
    Added,
    Known,
    MissingDeps,
}

/// One log in memory. Insertion order is a topological order, as in
/// `sync::Store`: a blob is only accepted once all its deps are stored.
#[derive(Default)]
pub struct MemStore {
    order: Vec<Id>,
    blobs: HashMap<Id, Blob>,
    heads: HashSet<Id>,
}

impl MemStore {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn heads(&self) -> Vec<Id> {
        let mut heads: Vec<Id> = self.heads.iter().copied().collect();
        heads.sort();
        heads
    }

    pub fn append(&mut self, blob: Blob) -> Append {
        if self.blobs.contains_key(&blob.id) {
            return Append::Known;
        }
        if blob.deps.iter().any(|d| !self.blobs.contains_key(d)) {
            return Append::MissingDeps;
        }
        for dep in &blob.deps {
            self.heads.remove(dep);
        }
        self.heads.insert(blob.id);
        self.order.push(blob.id);
        self.blobs.insert(blob.id, blob);
        Append::Added
    }

    /// Heads plus the blobs 1, 2, 4, 8, … back in insertion order.
    pub fn have_markers(&self) -> Vec<Id> {
        let mut have = self.heads();
        let mut offset = 1;
        while offset < self.order.len() {
            have.push(self.order[self.order.len() - 1 - offset]);
            offset *= 2;
        }
        have
    }

    /// Blobs a peer holding `have` (and their ancestors) lacks, in order.
    pub fn missing_since(&self, have: &[Id], limit: usize) -> (Vec<Blob>, bool) {
        let mut known: HashSet<Id> = HashSet::new();
        let mut stack: Vec<Id> =
            have.iter().filter(|id| self.blobs.contains_key(*id)).copied().collect();
        while let Some(id) = stack.pop() {
            if known.insert(id) {
                stack.extend(self.blobs[&id].deps.iter().copied());
            }
        }
        let mut out: Vec<Blob> = self
            .order
            .iter()
            .filter(|id| !known.contains(*id))
            .take(limit + 1)
            .map(|id| self.blobs[id].clone())
            .collect();
        let more = out.len() > limit;
        out.truncate(limit);
        (out, more)
    }

    /// Same as `sync::Store::state_hash`: BLAKE3 over the sorted ids.
    pub fn state_hash(&self) -> String {
        let mut ids = self.order.clone();
        ids.sort();
        let mut hasher = blake3::Hasher::new();
        for id in ids {
            hasher.update(&id);
        }
        hasher.finalize().to_hex().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memstore_pull_semantics() {
        let mut a = MemStore::default();
        let mut prev = vec![];
        for i in 0..10 {
            let blob = Blob::new(prev, format!("{i}").into_bytes());
            prev = vec![blob.id];
            assert_eq!(a.append(blob), Append::Added);
        }
        let (all, more) = a.missing_since(&[], 100);
        assert_eq!((all.len(), more), (10, false));
        let (rest, _) = a.missing_since(&[all[6].id], 100);
        assert_eq!(rest.len(), 3);
        let (page, more) = a.missing_since(&[], 4);
        assert_eq!((page.len(), more), (4, true));
        assert_eq!(a.missing_since(&a.have_markers(), 100).0.len(), 0);
        assert_eq!(a.append(Blob::new(vec![[9; 32]], vec![])), Append::MissingDeps);
    }
}
