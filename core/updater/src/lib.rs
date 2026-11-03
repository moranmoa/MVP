//! Spike experiment 7: the signed update chain.
//!
//! How bytes arrive (P2P, HTTPS, USB stick) does not matter here. Every rule
//! runs on bytes already in hand:
//!
//! 1. [`SignedManifest::verify`]: enough root-key signatures (1-of-1 in the
//!    closed alpha, 2-of-3 from the open alpha), known format, not expired.
//! 2. [`ClientPolicy::accept`]: right channel, not older than a manifest
//!    already seen, never a downgrade, never a revoked version.
//! 3. [`verify_artifact`]: size and SHA-256 match, before installing *and*
//!    before serving the artifact to other peers.
//! 4. [`InstallState`]: trial boot, health check, rollback to the previous
//!    version after N failed starts.

mod artifact;
mod error;
mod manifest;
mod policy;
mod rollback;
mod version;

pub use artifact::{hash_reader, verify_artifact, VerifiedArtifact};
pub use error::Error;
pub use manifest::{
    parse_public_key, Artifact, ManifestBody, SignatureEntry, SignedManifest, TrustRoot, TrustRootFile,
    VerifiedManifest, FORMAT, MAX_MANIFEST_BYTES, SIGNING_CONTEXT,
};
pub use policy::{ClientPolicy, Decision};
pub use rollback::{Boot, Committed, InstallState, Trial};
pub use version::{ParseVersionError, Version};

pub use ed25519_dalek::{SigningKey, VerifyingKey};
