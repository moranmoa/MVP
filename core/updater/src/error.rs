use std::fmt;

use crate::Version;

/// Every way an update can be refused. Each variant is one security rule, so
/// callers (and tests) can tell exactly which rule fired.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The trust root itself is unusable (bad threshold, duplicate or weak key).
    InvalidTrustRoot(String),
    /// The manifest bytes are not a well-formed signed manifest.
    Malformed(String),
    /// Fewer distinct trusted keys signed the manifest than the threshold.
    InsufficientSignatures { valid: usize, required: usize },
    /// `now` is at or past the manifest's `expires_at`.
    Expired { expires_at: u64, now: u64 },
    /// `issued_at` is further ahead of `now` than the clock-skew allowance.
    IssuedInFuture { issued_at: u64, now: u64 },
    /// The manifest is for another update channel.
    WrongChannel { expected: String, found: String },
    /// The manifest is older than one this client already accepted (replay).
    StaleManifest { seen: Version, offered: Version },
    /// The offered version is lower than the installed one.
    Downgrade { installed: Version, offered: Version },
    /// The version was revoked, by this manifest or an earlier one.
    Revoked(Version),
    /// The manifest has no artifact for this platform.
    NoArtifact(String),
    /// The artifact's size differs from the manifest.
    SizeMismatch { expected: u64, actual: u64 },
    /// The artifact's hash differs from the manifest.
    HashMismatch,
    /// A staged update is not newer than the running version.
    NotNewer { current: Version, offered: Version },
    /// The version already failed its health check here; it is not retried.
    PreviouslyFailed(Version),
    /// A health report arrived with no trial boot in progress.
    NoTrial,
    /// A root rotation skipped or repeated a version.
    RootVersion { expected: u64, found: u64 },
    /// The artifact was verified for another platform.
    WrongPlatform { expected: String, found: String },
    /// The update payload could not be unpacked safely (B8 §5, A3).
    Unpack(String),
    /// A persisted state file is inconsistent (corrupted or tampered with).
    InvalidState(String),
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidTrustRoot(why) => write!(f, "invalid trust root: {why}"),
            Error::Malformed(why) => write!(f, "malformed manifest: {why}"),
            Error::InsufficientSignatures { valid, required } => {
                write!(f, "not enough valid signatures: {valid} of {required} required")
            }
            Error::Expired { expires_at, now } => {
                write!(f, "manifest expired at {expires_at} (now {now})")
            }
            Error::IssuedInFuture { issued_at, now } => {
                write!(f, "manifest issued at {issued_at}, more than a day after now ({now}); check the clock")
            }
            Error::WrongChannel { expected, found } => {
                write!(f, "manifest is for channel {found:?}, client is on {expected:?}")
            }
            Error::StaleManifest { seen, offered } => {
                write!(f, "manifest for {offered} is older than already accepted {seen}")
            }
            Error::Downgrade { installed, offered } => {
                write!(f, "refusing downgrade from {installed} to {offered}")
            }
            Error::Revoked(v) => write!(f, "version {v} is revoked"),
            Error::NoArtifact(p) => write!(f, "no artifact for platform {p:?}"),
            Error::SizeMismatch { expected, actual } => {
                write!(f, "artifact size {actual} does not match manifest size {expected}")
            }
            Error::HashMismatch => write!(f, "artifact hash does not match manifest"),
            Error::NotNewer { current, offered } => {
                write!(f, "staged version {offered} is not newer than {current}")
            }
            Error::PreviouslyFailed(v) => write!(f, "version {v} already failed its health check"),
            Error::NoTrial => write!(f, "no trial boot in progress"),
            Error::RootVersion { expected, found } => {
                write!(f, "root update has version {found}, expected {expected}")
            }
            Error::WrongPlatform { expected, found } => {
                write!(f, "artifact is for platform {found:?}, this install is {expected:?}")
            }
            Error::InvalidState(why) => write!(f, "invalid persisted state: {why}"),
            Error::Unpack(why) => write!(f, "unsafe update payload: {why}"),
            Error::Io(why) => write!(f, "i/o error: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
