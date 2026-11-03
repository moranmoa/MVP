//! A user's identity, made from 12 recovery words.
//!
//! - [`RecoveryPhrase`]: the 12 BIP-39 words (128 bits of entropy plus a
//!   4-bit checksum), with typo hints for the restore screen.
//! - [`Identity`]: the keys derived from the words: the Ed25519 identity key,
//!   the local storage key, and per-purpose subkeys.
//! - [`SecretStore`]: where the secret lives on this device. [`OsKeychain`]
//!   is the real one, [`MemoryStore`] is for tests.
//! - [`Vault`]: create, restore, app lock, and the recovery-word check.
//!
//! See README.md for the derivation tree and the security properties.
//!
//! No type here prints a secret through `Debug`, and nothing is logged.

mod derive;
mod lock;
mod phrase;
mod store;
mod vault;

pub use derive::{Identity, StorageKey, Subkey};
pub use lock::LockParams;
pub use phrase::{
    check_positions, is_word, suggest, word_list, PhraseError, RecoveryPhrase, UnknownWord,
    WORD_COUNT,
};
#[cfg(feature = "os-keychain")]
pub use store::OsKeychain;
pub use store::{MemoryStore, SecretStore, StoreError, KEYCHAIN_SERVICE};
pub use vault::{Vault, VaultError, VaultState, ENTRY_RECOVERY_CHECK, ENTRY_SECRET};
