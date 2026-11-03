//! Where secrets live on this device.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use zeroize::Zeroizing;

/// A small store of named secrets. Implementations never log values.
pub trait SecretStore {
    /// `None` if there is no entry with this name.
    fn load(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError>;
    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError>;
    /// Succeeds if the entry is already gone.
    fn delete(&self, name: &str) -> Result<(), StoreError>;
}

/// The platform store failed (locked keychain, no Secret Service running,
/// access denied). The message never contains a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "secret store: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// In-memory store for tests. Values are zeroized when replaced or dropped.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl SecretStore for MemoryStore {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        let entries = self.entries.lock().expect("not poisoned");
        Ok(entries.get(name).cloned())
    }

    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError> {
        let mut entries = self.entries.lock().expect("not poisoned");
        entries.insert(name.to_owned(), Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.entries.lock().expect("not poisoned").remove(name);
        Ok(())
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries = self.entries.lock().expect("not poisoned");
        let mut names: Vec<&String> = entries.keys().collect();
        names.sort();
        f.debug_struct("MemoryStore").field("names", &names).finish()
    }
}

impl<S: SecretStore + ?Sized> SecretStore for &S {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        (**self).load(name)
    }

    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError> {
        (**self).save(name, secret)
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        (**self).delete(name)
    }
}

/// The OS keychain: Windows Credential Manager, macOS Keychain, or the
/// Secret Service on Linux (GNOME Keyring, KWallet). Entries are
/// (`service`, name) pairs.
#[cfg(feature = "os-keychain")]
#[derive(Clone, Debug)]
pub struct OsKeychain {
    service: String,
}

#[cfg(feature = "os-keychain")]
impl OsKeychain {
    pub fn new(service: impl Into<String>) -> OsKeychain {
        OsKeychain { service: service.into() }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, name).map_err(platform)
    }
}

#[cfg(feature = "os-keychain")]
impl SecretStore for OsKeychain {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        match self.entry(name)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(platform(e)),
        }
    }

    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError> {
        self.entry(name)?.set_secret(secret).map_err(platform)
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(platform(e)),
        }
    }
}

#[cfg(feature = "os-keychain")]
fn platform(error: keyring::Error) -> StoreError {
    match error {
        // These two carry the stored bytes; never put them in a message.
        keyring::Error::BadEncoding(_) | keyring::Error::BadDataFormat(..) => {
            StoreError("stored value has an unexpected format".into())
        }
        other => StoreError(other.to_string()),
    }
}
