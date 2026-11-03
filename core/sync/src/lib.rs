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
//!
//! Everything a peer sends is untrusted. A blob's id is never taken from the
//! wire: it is recomputed from the log, deps and data ([`Blob::content_id`]),
//! so a peer can't store junk under the id of a real record, or a real record
//! under another log.

use std::fmt;

mod limits;
pub mod node;
pub mod store;

pub use node::{Node, NodeConfig, NodeEvent, Via};
pub use store::{Append, Store};

pub type Id = [u8; 32];

/// Most deps a blob may have; same as `records::MAX_DEPS`.
pub const MAX_DEPS: usize = 64;
/// Largest blob data. A record is at most ~19 KiB (`records::MAX_RECORD_BYTES`).
pub const MAX_DATA_BYTES: usize = 32 * 1024;
/// Largest encoded blob on the wire.
pub const MAX_ENCODED_BYTES: usize = 2 + MAX_DEPS * 32 + MAX_DATA_BYTES;
/// Longest log name, in bytes.
pub const MAX_LOG_NAME: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub id: Id,
    pub deps: Vec<Id>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobError {
    Truncated,
    TooManyDeps(usize),
    TooLarge(usize),
    /// The id isn't the hash of the blob's log, deps and data.
    WrongId,
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobError::Truncated => write!(f, "truncated blob"),
            BlobError::TooManyDeps(n) => write!(f, "{n} deps, limit is {MAX_DEPS}"),
            BlobError::TooLarge(n) => write!(f, "{n} bytes of data, limit is {MAX_DATA_BYTES}"),
            BlobError::WrongId => write!(f, "id does not match the content"),
        }
    }
}

impl std::error::Error for BlobError {}

/// A log name: 1–128 printable ASCII characters, no spaces. Log names go into
/// gossipsub topic names and SQL parameters (never into SQL text).
pub fn valid_log_name(log: &str) -> bool {
    !log.is_empty() && log.len() <= MAX_LOG_NAME && log.bytes().all(|b| b.is_ascii_graphic())
}

impl Blob {
    /// A blob of `log` on top of `deps`, with its content id.
    pub fn new(log: &str, deps: Vec<Id>, data: Vec<u8>) -> Self {
        Self { id: Self::content_id(log, &deps, &data), deps, data }
    }

    /// The id a blob must have: BLAKE3 over the log name, deps and data.
    ///
    /// Hook for `core/records`: once blobs are real `records::Record`s, this
    /// becomes `Record::decode(data)?.id()` (which also checks the signature),
    /// plus checks that the record's scope is `log` and its deps are `deps`.
    pub fn content_id(log: &str, deps: &[Id], data: &[u8]) -> Id {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mvp/sync-blob/v1\0");
        hasher.update(&(log.len() as u32).to_le_bytes());
        hasher.update(log.as_bytes());
        hasher.update(&(deps.len() as u32).to_le_bytes());
        for dep in deps {
            hasher.update(dep);
        }
        hasher.update(data);
        *hasher.finalize().as_bytes()
    }

    /// Checks the limits and that the id matches the content. Every blob that
    /// reaches the store passes through here.
    pub fn verify(&self, log: &str) -> Result<(), BlobError> {
        if self.deps.len() > MAX_DEPS {
            return Err(BlobError::TooManyDeps(self.deps.len()));
        }
        if self.data.len() > MAX_DATA_BYTES {
            return Err(BlobError::TooLarge(self.data.len()));
        }
        if Self::content_id(log, &self.deps, &self.data) != self.id {
            return Err(BlobError::WrongId);
        }
        Ok(())
    }

    /// Wire format: dep count (u16 LE), deps, data. The id is not sent; the
    /// receiver computes it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + 32 * self.deps.len() + self.data.len());
        out.extend_from_slice(&(self.deps.len() as u16).to_le_bytes());
        for dep in &self.deps {
            out.extend_from_slice(dep);
        }
        out.extend_from_slice(&self.data);
        out
    }

    /// Decodes a blob received for `log`, enforcing the limits.
    pub fn decode(log: &str, bytes: &[u8]) -> Result<Self, BlobError> {
        let count: [u8; 2] = bytes.get(..2).and_then(|b| b.try_into().ok()).ok_or(BlobError::Truncated)?;
        let n = u16::from_le_bytes(count) as usize;
        if n > MAX_DEPS {
            return Err(BlobError::TooManyDeps(n));
        }
        let deps_end = 2 + 32 * n;
        let deps = bytes.get(2..deps_end).ok_or(BlobError::Truncated)?;
        let data = &bytes[deps_end..];
        if data.len() > MAX_DATA_BYTES {
            return Err(BlobError::TooLarge(data.len()));
        }
        let deps: Vec<Id> = deps.chunks_exact(32).map(|c| c.try_into().expect("chunks are 32 bytes")).collect();
        Ok(Self::new(log, deps, data.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_roundtrip() {
        let a = Blob::new("l", vec![], b"a".to_vec());
        let b = Blob::new("l", vec![a.id, [7; 32]], "שלום".as_bytes().to_vec());
        assert_eq!(Blob::decode("l", &b.encode()), Ok(b));
        assert_eq!(Blob::decode("l", &a.encode()), Ok(a));
        assert_eq!(Blob::decode("l", &[1]), Err(BlobError::Truncated));
        // Claims one dep but carries none.
        assert_eq!(Blob::decode("l", &[1, 0, 9]), Err(BlobError::Truncated));
    }

    #[test]
    fn forged_id_is_rejected() {
        let real = Blob::new("l", vec![], b"real".to_vec());
        let forged = Blob { id: real.id, deps: vec![], data: b"junk".to_vec() };
        assert_eq!(forged.verify("l"), Err(BlobError::WrongId));
        assert_eq!(real.verify("l"), Ok(()));
        // The same content under another log has another id.
        assert_eq!(real.verify("other"), Err(BlobError::WrongId));
        assert_ne!(Blob::decode("other", &real.encode()).unwrap().id, real.id);
    }

    #[test]
    fn decode_enforces_limits() {
        // A huge dep count is rejected before anything is allocated.
        let mut bytes = (u16::MAX).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 64]);
        assert_eq!(Blob::decode("l", &bytes), Err(BlobError::TooManyDeps(u16::MAX as usize)));

        let max = Blob::new("l", vec![[1; 32]; MAX_DEPS], vec![0; MAX_DATA_BYTES]);
        assert_eq!(max.encode().len(), MAX_ENCODED_BYTES);
        assert!(Blob::decode("l", &max.encode()).is_ok());

        let big = Blob::new("l", vec![], vec![0; MAX_DATA_BYTES + 1]);
        assert_eq!(Blob::decode("l", &big.encode()), Err(BlobError::TooLarge(MAX_DATA_BYTES + 1)));
        assert_eq!(big.verify("l"), Err(BlobError::TooLarge(MAX_DATA_BYTES + 1)));
        let many = Blob::new("l", vec![[1; 32]; MAX_DEPS + 1], vec![]);
        assert_eq!(many.verify("l"), Err(BlobError::TooManyDeps(MAX_DEPS + 1)));
    }

    #[test]
    fn log_names() {
        assert!(valid_log_name("general"));
        assert!(valid_log_name("channel/3f9a"));
        assert!(!valid_log_name(""));
        assert!(!valid_log_name("has space"));
        assert!(!valid_log_name("קבוצה"));
        assert!(!valid_log_name(&"x".repeat(MAX_LOG_NAME + 1)));
    }
}
