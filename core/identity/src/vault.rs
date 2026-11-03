//! The identity on this device: create, restore, app lock, recovery check.

use std::fmt;

use crate::lock::{self, Decoded, LockError, LockParams};
use crate::phrase::RecoveryPhrase;
use crate::store::{SecretStore, StoreError};

/// The 128-bit entropy, plain or app-locked. Everything else is derived.
const ENTRY_SECRET: &str = "identity";
/// Present once the user has passed the recovery-word check. Not secret.
const ENTRY_CHECKED: &str = "recovery-check-done";

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
    EmptyPassphrase,
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
    /// cost they were made with.
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
        let phrase = RecoveryPhrase::generate();
        self.store.save(ENTRY_SECRET, &lock::encode_plain(&phrase.entropy()))?;
        self.store.delete(ENTRY_CHECKED)?;
        Ok(phrase)
    }

    /// Restore on a new device. Typing all 12 words counts as passing the
    /// recovery-word check.
    pub fn restore(&self, phrase: &RecoveryPhrase) -> Result<(), VaultError> {
        self.ensure_empty()?;
        self.store.save(ENTRY_SECRET, &lock::encode_plain(&phrase.entropy()))?;
        self.store.save(ENTRY_CHECKED, &[1])?;
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
    /// passphrase in use, `None` if there is no lock yet.
    pub fn set_app_lock(&self, current: Option<&str>, new: &str) -> Result<(), VaultError> {
        if new.is_empty() {
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

    pub fn recovery_check_done(&self) -> Result<bool, VaultError> {
        Ok(self.store.load(ENTRY_CHECKED)?.is_some())
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
            self.store.save(ENTRY_CHECKED, &[1])?;
        }
        Ok(ok)
    }

    /// Call before creating a server: the check can be postponed, but not
    /// past that.
    pub fn require_recovery_check(&self) -> Result<(), VaultError> {
        if self.recovery_check_done()? {
            Ok(())
        } else {
            Err(VaultError::RecoveryCheckRequired)
        }
    }

    /// Deletes the identity from this device. Only the words bring it back.
    pub fn forget(&self) -> Result<(), VaultError> {
        self.store.delete(ENTRY_SECRET)?;
        self.store.delete(ENTRY_CHECKED)?;
        Ok(())
    }

    fn ensure_empty(&self) -> Result<(), VaultError> {
        match self.store.load(ENTRY_SECRET)? {
            None => Ok(()),
            Some(_) => Err(VaultError::AlreadyExists),
        }
    }
}
