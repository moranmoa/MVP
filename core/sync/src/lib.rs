//! Log sync between peers (spike experiment 2).
//!
//! Records are treated as opaque content-addressed blobs: an id, the ids
//! they depend on, and bytes. The real record format lives in `core/records`;
//! this crate only needs those three things from it.
//!
//! - [`Store`]: SQLite store with `append`, `get`, `heads`, `missing_since`.
//! - [`node`]: a libp2p node that finds peers through the bootstrap/relay
//!   node, pulls what it is missing (heads + markers → missing blobs, paged)
//!   and pushes new blobs live over gossipsub.

pub mod node;
pub mod store;

pub use node::{Node, NodeConfig, NodeEvent, Via};
pub use store::{Append, Store};

pub type Id = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub id: Id,
    pub deps: Vec<Id>,
    pub data: Vec<u8>,
}

impl Blob {
    /// A blob whose id is the BLAKE3 hash of its deps and data. Used by the
    /// tests and the harness; real records bring their own ids.
    pub fn new(deps: Vec<Id>, data: Vec<u8>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(deps.len() as u32).to_le_bytes());
        for dep in &deps {
            hasher.update(dep);
        }
        hasher.update(&data);
        Self { id: *hasher.finalize().as_bytes(), deps, data }
    }

    /// Wire format: id, dep count (u16 LE), deps, data.
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
        let deps = bytes
            .get(34..deps_end)?
            .chunks_exact(32)
            .map(|c| c.try_into().unwrap())
            .collect();
        Some(Self { id, deps, data: bytes[deps_end..].to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_roundtrip() {
        let a = Blob::new(vec![], b"a".to_vec());
        let b = Blob::new(vec![a.id, [7; 32]], "שלום".as_bytes().to_vec());
        assert_eq!(Blob::decode(&b.encode()), Some(b));
        assert_eq!(Blob::decode(&a.encode()), Some(a));
        assert_eq!(Blob::decode(&[1, 2, 3]), None);
    }
}
