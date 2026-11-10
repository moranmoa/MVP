//! The identity on this device: create, restore, app lock, recovery check.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::derive::Identity;
use crate::lock::{self, Decoded, LockError, LockParams};
use crate::phrase::RecoveryPhrase;
use crate::store::{SecretStore, StoreError};

/// Entry holding the 128-bit entropy, plain or app-locked. Everything else
/// is derived. Frozen for the alpha.
pub const ENTRY_SECRET: &str = "identity-secret-v1";
/// Entry written once the user passes the recovery-word check. Holds a tag
/// of the identity it was passed for (not secret), so a flag left over from
/// another identity never counts. Frozen for the alpha.
pub const ENTRY_RECOVERY_CHECK: &str = "identity-recovery-check-v1";
const CHECK_TAG_DOMAIN: &[u8] = b"mvs/identity/recovery-check/v1";
const CHECK_TAG_LEN: usize = 16;

/// First 16 bytes of SHA-256(domain || identity public key).
fn check_tag(identity: &Identity) -> [u8; CHECK_TAG_LEN] {
    let digest = Sha256::new()
        .chain_update(CHECK_TAG_DOMAIN)
        .chain_update(identity.public_key().as_bytes())
        .finalize();
    let mut tag = [0u8; CHECK_TAG_LEN];
    tag.copy_from_slice(&digest[..CHECK_TAG_LEN]);
    tag
}

/// What the store holds right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultState {
    /// No identity on this device: show sign-up or restore.
    Empty,
    /// An identity without app lock: [`Vault::open`] works.
    Ready,
    /// An identity behind the app lock: [`Vault::unlock`] with the passphrase.
    Locked,
}

#[derive(Clone, PartialEq, Eq)]
pub enum VaultError {
    Store(StoreError),
    /// There is already an identity; [`Vault::forget`] it first.
    AlreadyExists,
    NotFound,
    /// The app lock is on; use [`Vault::unlock`].
    Locked,
    WrongPassphrase,
    /// Empty or only whitespace.
    EmptyPassphrase,
    /// [`Vault::with_lock_params`] was given a cost below [`LockParams::MIN`]
    /// (or above what unlocking accepts).
    WeakLockParams,
    /// The stored entry is unreadable.
    Corrupt,
    /// The user has not passed the recovery-word check yet. Creating a
    /// server needs it.
    RecoveryCheckRequired,
}

impl fmt::Debug for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Store(e) => write!(f, "{e}"),
            VaultError::AlreadyExists => f.write_str("an identity already exists on this device"),
            VaultError::NotFound => f.write_str("no identity on this device"),
            VaultError::Locked => f.write_str("the app lock is on"),
            VaultError::WrongPassphrase => f.write_str("wrong app lock passphrase"),
            VaultError::EmptyPassphrase => f.write_str("the app lock passphrase is empty"),
            VaultError::WeakLockParams => f.write_str("app lock cost is outside the allowed range"),
            VaultError::Corrupt => f.write_str("the stored identity is unreadable"),
            VaultError::RecoveryCheckRequired => f.write_str("recovery-word check required"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<StoreError> for VaultError {
    fn from(e: StoreError) -> VaultError {
        VaultError::Store(e)
    }
}

impl From<LockError> for VaultError {
    fn from(e: LockError) -> VaultError {
        match e {
            LockError::Corrupt => VaultError::Corrupt,
            LockError::WrongPassphrase => VaultError::WrongPassphrase,
            LockError::WeakParams => VaultError::WeakLockParams,
        }
    }
}

/// The identity secret in a [`SecretStore`].
#[derive(Debug)]
pub struct Vault<S> {
    store: S,
    params: LockParams,
}

impl<S: SecretStore> Vault<S> {
    pub fn new(store: S) -> Vault<S> {
        Vault { store, params: LockParams::default() }
    }

    /// Argon2id cost for app locks set from now on. Existing locks keep the
    /// cost they were made with. Below [`LockParams::MIN`], setting a lock
    /// fails with [`VaultError::WeakLockParams`].
    pub fn with_lock_params(mut self, params: LockParams) -> Vault<S> {
        self.params = params;
        self
    }

    pub fn state(&self) -> Result<VaultState, VaultError> {
        match self.store.load(ENTRY_SECRET)? {
            None => Ok(VaultState::Empty),
            Some(blob) => match lock::decode(&blob)? {
                Decoded::Plain(_) => Ok(VaultState::Ready),
                Decoded::Locked => Ok(VaultState::Locked),
            },
        }
    }

    /// Sign-up: new random words, stored without app lock. The
    /// recovery-word check is still pending; show the words, then call
    /// [`Vault::confirm_recovery_check`] (now or after a reminder).
    pub fn create(&self) -> Result<RecoveryPhrase, VaultError> {
        self.ensure_empty()?;
        // Flag first: if this fails, nothing is stored yet.
        self.store.delete(ENTRY_RECOVERY_CHECK)?;
        let phrase = RecoveryPhrase::generate();
        self.store.save(ENTRY_SECRET, &lock::encode_plain(&phrase.entropy()))?;
        Ok(phrase)
    }

    /// Restore on a new device. Typing all 12 words counts as passing the
    /// recovery-word check.
    pub fn restore(&self, phrase: &RecoveryPhrase) -> Result<(), VaultError> {
        self.ensure_empty()?;
        self.store.save(ENTRY_SECRET, &lock::encode_plain(&phrase.entropy()))?;
        self.store.save(ENTRY_RECOVERY_CHECK, &check_tag(&phrase.identity()))?;
        Ok(())
    }

    /// The words, when there is no app lock.
    pub fn open(&self) -> Result<RecoveryPhrase, VaultError> {
        let blob = self.store.load(ENTRY_SECRET)?.ok_or(VaultError::NotFound)?;
        match lock::decode(&blob)? {
            Decoded::Plain(entropy) => Ok(RecoveryPhrase::from_entropy(&entropy)),
            Decoded::Locked => Err(VaultError::Locked),
        }
    }

    /// The words, behind the app lock. Also works when there is no lock.
    pub fn unlock(&self, passphrase: &str) -> Result<RecoveryPhrase, VaultError> {
        let blob = self.store.load(ENTRY_SECRET)?.ok_or(VaultError::NotFound)?;
        match lock::decode(&blob)? {
            Decoded::Plain(entropy) => Ok(RecoveryPhrase::from_entropy(&entropy)),
            Decoded::Locked => Ok(RecoveryPhrase::from_entropy(&*lock::open(&blob, passphrase)?)),
        }
    }

    /// Turns the app lock on, or changes its passphrase. `current` is the
    /// passphrase in use, `None` if there is no lock yet. Passphrases are
    /// compared after Unicode NFC, so the same Hebrew text typed on another
    /// OS still unlocks.
    pub fn set_app_lock(&self, current: Option<&str>, new: &str) -> Result<(), VaultError> {
        if new.trim().is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }
        let phrase = match current {
            Some(current) => self.unlock(current)?,
            None => self.open()?,
        };
        let sealed = lock::seal(&phrase.entropy(), new, self.params)?;
        self.store.save(ENTRY_SECRET, &sealed)?;
        Ok(())
    }

    pub fn remove_app_lock(&self, current: &str) -> Result<(), VaultError> {
        let phrase = self.unlock(current)?;
        self.store.save(ENTRY_SECRET, &lock::encode_plain(&phrase.entropy()))?;
        Ok(())
    }

    /// Whether the check was passed for this `identity`. A flag left by
    /// another identity does not count.
    pub fn recovery_check_done(&self, identity: &Identity) -> Result<bool, VaultError> {
        let stored = self.store.load(ENTRY_RECOVERY_CHECK)?;
        Ok(stored.is_some_and(|tag| tag.as_slice() == check_tag(identity)))
    }

    /// Records the check as passed if every answer matches. `answers` are
    /// (0-based position, typed word), e.g. for [`crate::check_positions`].
    pub fn confirm_recovery_check(
        &self,
        phrase: &RecoveryPhrase,
        answers: &[(usize, &str)],
    ) -> Result<bool, VaultError> {
        let ok = phrase.check_words(answers);
        if ok {
            self.store.save(ENTRY_RECOVERY_CHECK, &check_tag(&phrase.identity()))?;
        }
        Ok(ok)
    }

    /// Call before creating a server: the check can be postponed, but not
    /// past that.
    pub fn require_recovery_check(&self, identity: &Identity) -> Result<(), VaultError> {
        if self.recovery_check_done(identity)? {
            Ok(())
        } else {
            Err(VaultError::RecoveryCheckRequired)
        }
    }

    /// Deletes the identity from this device. Only the words bring it back.
    /// Sign-out: deletes this identity from the keychain. The secret goes
    /// first: if that fails nothing changed, the identity still opens, and
    /// that is the only error. Once the secret is gone the identity is gone,
    /// so a failure to delete the recovery-check marker after it is not an
    /// error: it returns `Ok(false)` ("marker left"). A stale marker only
    /// says "checked" if the same words are restored here.
    pub fn forget(&self) -> Result<bool, VaultError> {
        self.store.delete(ENTRY_SECRET)?;
        Ok(self.store.delete(ENTRY_RECOVERY_CHECK).is_ok())
    }

    fn ensure_empty(&self) -> Result<(), VaultError> {
        match self.store.load(ENTRY_SECRET)? {
            None => Ok(()),
            Some(_) => Err(VaultError::AlreadyExists),
        }
    }
}
