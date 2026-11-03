//! The core/sync wire format, buildable for the browser (wasm32).
//!
//! - [`Blob`]: byte-for-byte the same as `sync::Blob` (u16 LE dep count,
//!   deps, data; the id is not sent but recomputed from log, deps and data;
//!   deps strictly ascending; same size limits).
//! - [`SyncRequest`] / [`SyncResponse`]: the CBOR request-response messages
//!   of `/mvs/sync/0.1.0`.
//! - [`gossip_message_id`]: core/sync's content-based gossipsub message id.
//! - [`MemStore`]: an in-memory log with the same pull semantics as
//!   `sync::Store` (have markers, missing-since, state hash), for the browser.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

pub const SYNC_PROTOCOL: &str = "/mvs/sync/0.1.0";
pub const IDENTIFY_VERSION: &str = "/mvs/0.1.0";
/// Blobs and blob bytes per sync response page (same as core/sync).
pub const PAGE: usize = 2000;
pub const PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Limits, same as core/sync.
pub const MAX_DEPS: usize = 64;
pub const MAX_DATA_BYTES: usize = 32 * 1024;
pub const MAX_ENCODED_BYTES: usize = 2 + MAX_DEPS * 32 + MAX_DATA_BYTES;
pub const MAX_GOSSIP_BYTES: usize = MAX_ENCODED_BYTES + 4 * 1024;
pub const MAX_HAVE: usize = 64;
const MAX_HAVE_HEADS: usize = 32;

pub type Id = [u8; 32];

pub fn topic(log: &str) -> String {
    format!("log/{log}")
}

/// core/sync dedups gossip by content: BLAKE3 of topic, a zero byte, data.
pub fn gossip_message_id(topic: &str, data: &[u8]) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic.as_bytes());
    hasher.update(&[0]);
    hasher.update(data);
    hasher.finalize().as_bytes().to_vec()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub id: Id,
    pub deps: Vec<Id>,
    pub data: Vec<u8>,
}

impl Blob {
    /// A blob of `log` on top of `deps` (sorted and deduplicated here).
    pub fn new(log: &str, mut deps: Vec<Id>, data: Vec<u8>) -> Self {
        deps.sort_unstable();
        deps.dedup();
        Self { id: Self::content_id(log, &deps, &data), deps, data }
    }

    pub fn content_id(log: &str, deps: &[Id], data: &[u8]) -> Id {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mvs/sync-blob/v1\0");
        hasher.update(&(log.len() as u32).to_le_bytes());
        hasher.update(log.as_bytes());
        hasher.update(&(deps.len() as u32).to_le_bytes());
        for dep in deps {
            hasher.update(dep);
        }
        hasher.update(data);
        *hasher.finalize().as_bytes()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + 32 * self.deps.len() + self.data.len());
        out.extend_from_slice(&(self.deps.len() as u16).to_le_bytes());
        for dep in &self.deps {
            out.extend_from_slice(dep);
        }
        out.extend_from_slice(&self.data);
        out
    }

    /// Decodes a blob received for `log`; `None` if it breaks a limit or its
    /// deps aren't strictly ascending (core/sync rejects those too).
    pub fn decode(log: &str, bytes: &[u8]) -> Option<Self> {
        let n = u16::from_le_bytes(bytes.get(..2)?.try_into().ok()?) as usize;
        if n > MAX_DEPS {
            return None;
        }
        let deps_end = 2 + 32 * n;
        let deps: Vec<Id> =
            bytes.get(2..deps_end)?.chunks_exact(32).map(|c| c.try_into().unwrap()).collect();
        let data = bytes[deps_end..].to_vec();
        if data.len() > MAX_DATA_BYTES || !deps.windows(2).all(|w| w[0] < w[1]) {
            return None;
        }
        Some(Self { id: Self::content_id(log, &deps, &data), deps, data })
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

    /// Up to 32 heads plus the blobs 1, 2, 4, 8, … back in insertion order,
    /// at most [`MAX_HAVE`] ids.
    pub fn have_markers(&self) -> Vec<Id> {
        let mut have = self.heads();
        have.truncate(MAX_HAVE_HEADS);
        let mut offset = 1;
        while have.len() < MAX_HAVE && offset < self.order.len() {
            have.push(self.order[self.order.len() - 1 - offset]);
            offset *= 2;
        }
        have
    }

    /// Blobs a peer holding `have` (and their ancestors) lacks, in order: at
    /// most `max_blobs` and about `max_bytes` (at least one blob if any).
    pub fn missing_since(
        &self,
        have: &[Id],
        max_blobs: usize,
        max_bytes: usize,
    ) -> (Vec<Blob>, bool) {
        let mut known: HashSet<Id> = HashSet::new();
        let mut stack: Vec<Id> =
            have.iter().take(MAX_HAVE).filter(|id| self.blobs.contains_key(*id)).copied().collect();
        while let Some(id) = stack.pop() {
            if known.insert(id) {
                stack.extend(self.blobs[&id].deps.iter().copied());
            }
        }
        let (mut out, mut bytes) = (Vec::new(), 0);
        for id in self.order.iter().filter(|id| !known.contains(*id)) {
            let blob = &self.blobs[id];
            let size = blob.data.len() + 32 * blob.deps.len();
            if out.len() >= max_blobs || (bytes + size > max_bytes && !out.is_empty()) {
                return (out, true);
            }
            bytes += size;
            out.push(blob.clone());
        }
        (out, false)
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
            let blob = Blob::new("l", prev, format!("{i}").into_bytes());
            prev = vec![blob.id];
            assert_eq!(a.append(blob), Append::Added);
        }
        let (all, more) = a.missing_since(&[], 100, PAGE_BYTES);
        assert_eq!((all.len(), more), (10, false));
        let (rest, _) = a.missing_since(&[all[6].id], 100, PAGE_BYTES);
        assert_eq!(rest.len(), 3);
        let (page, more) = a.missing_since(&[], 4, PAGE_BYTES);
        assert_eq!((page.len(), more), (4, true));
        let (page, more) = a.missing_since(&[], 100, 1);
        assert_eq!((page.len(), more), (1, true));
        assert_eq!(a.missing_since(&a.have_markers(), 100, PAGE_BYTES).0.len(), 0);
        assert_eq!(a.append(Blob::new("l", vec![[9; 32]], vec![])), Append::MissingDeps);
    }

    #[test]
    fn decode_rejects_unsorted_deps() {
        let mut bytes = 2u16.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[2; 32]);
        bytes.extend_from_slice(&[1; 32]);
        assert_eq!(Blob::decode("l", &bytes), None);
    }
}
