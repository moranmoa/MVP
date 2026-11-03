//! Spike experiment 7: the signed update chain.
//!
//! How bytes arrive (P2P, HTTPS, USB stick) does not matter here. Every rule
//! runs on bytes already in hand:
//!
//! 1. [`TrustRoot::update_chain`]: apply root-key rotations, each signed by
//!    the old and the new keys.
//! 2. [`SignedManifest::verify`]: enough root-key signatures (1-of-1 in the
//!    closed alpha, 2-of-3 from the open alpha), known format, not expired.
//! 3. [`ClientPolicy::accept`]: right channel, not expired against the
//!    signed time floor, not older than a manifest already seen, never a
//!    downgrade, never a revoked version.
//! 4. [`store_artifact`] / [`StoredArtifact::open_verified`]: size and
//!    SHA-256 match, before installing *and* before serving the artifact to
//!    other peers, read through one handle so the file cannot be swapped.
//! 5. [`InstallState`]: trial boot, health check, rollback to the previous
//!    version after N failed starts.
//!
//! Threat model and audit notes: `SECURITY.md` in this crate.

mod artifact;
mod error;
mod manifest;
mod policy;
mod rollback;
mod unpack;
mod root;
mod version;

pub use artifact::{hash_reader, store_artifact, verify_artifact, StoredArtifact, VerifiedArtifact};
pub use error::Error;
pub use manifest::{
    is_valid_name, Artifact, ManifestBody, SignatureEntry, SignedManifest, VerifiedManifest, FORMAT,
    MAX_CLOCK_SKEW_SECS, MAX_MANIFEST_BYTES, MAX_NOTES_BYTES, MAX_NOTES_LANGUAGES, MAX_SIGNATURES, MAX_VALIDITY_SECS,
    SIGNING_CONTEXT,
};
pub use policy::{Baseline, ClientPolicy, Decision, Freshness};
pub use rollback::{Boot, Committed, InstallState, Trial, MAX_ATTEMPTS};
pub use root::{
    parse_public_key, parse_root_fetch, RootChain, SignedRoot, TrustRoot, TrustRootFile, MAX_ROOT_FETCH_BYTES,
    MAX_ROOT_UPDATES_PER_FETCH, MAX_ROOT_UPDATE_BYTES, ROOT_SIGNING_CONTEXT,
};
pub use unpack::{
    check_entry_name, unpack_zip, MAX_COMPONENT_BYTES, MAX_ENTRIES, MAX_EXPANSION, MAX_PATH_BYTES, MAX_UNPACKED_BYTES,
};
pub use version::{ParseVersionError, Version};

pub use ed25519_dalek::{SigningKey, VerifyingKey};
