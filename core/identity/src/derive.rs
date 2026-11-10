//! Keys derived from the recovery words. See README.md for the tree.

use std::fmt;

use ed25519_dalek::{SigningKey, VerifyingKey};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

/// HKDF-Extract salt. Frozen for the alpha: changing it changes every
/// user's identity.
const SALT: &[u8] = b"mvs/identity/v1";
/// HKDF-Expand labels. Each purpose has its own; none is a prefix of another
/// once the fixed-length suffixes are counted.
const INFO_IDENTITY: &[u8] = b"identity/ed25519";
const INFO_STORAGE: &[u8] = b"local-storage/key";
const INFO_DEVICE: &[u8] = b"device/ed25519/";
const INFO_BACKUP_KEY: &[u8] = b"backup/key";
const INFO_BACKUP_WRITE: &[u8] = b"backup/write/ed25519";
/// BLAKE3 `derive_key` context for the backup tag (design note
/// `docs/design/restore-servers-from-network.md`). Holders compute it from
/// the public write key, so it stays public and frozen.
pub const BACKUP_TAG_CONTEXT: &str = "mvs/backup-tag/v1";

const KEY_LEN: usize = 32;

/// Further keys derived from the same words. New purposes get new variants
/// and new labels; existing ones never change.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subkey {
    /// An Ed25519 key for device number `n`. The identity key signs a
    /// certificate for it (linking devices, a later stage).
    Device(u32),
}

impl Subkey {
    fn info(self) -> Vec<u8> {
        match self {
            Subkey::Device(n) => [INFO_DEVICE, &n.to_be_bytes()].concat(),
        }
    }
}

/// Every key of one user, derived from their recovery words. The secret
/// parts are zeroized on drop; `Debug` shows only the public key.
pub struct Identity {
    signing: SigningKey,
    storage: StorageKey,
    prk: Zeroizing<[u8; KEY_LEN]>,
}

impl Identity {
    pub(crate) fn from_seed(seed: &[u8; 64]) -> Identity {
        let (mut prk, _) = Hkdf::<Sha256>::extract(Some(SALT), seed);
        let mut prk_bytes = Zeroizing::new([0u8; KEY_LEN]);
        prk_bytes.copy_from_slice(&prk);
        prk.as_mut_slice().zeroize();
        let signing = SigningKey::from_bytes(&expand(&prk_bytes, INFO_IDENTITY));
        let storage = StorageKey(expand(&prk_bytes, INFO_STORAGE));
        Identity { signing, storage, prk: prk_bytes }
    }

    /// The identity key, for `records::Draft::sign`.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// The user's ID.
    pub fn public_key(&self) -> records::PublicKey {
        records::PublicKey::from_bytes(self.signing.verifying_key().to_bytes())
    }

    /// The key that encrypts this user's local storage.
    pub fn storage_key(&self) -> &StorageKey {
        &self.storage
    }

    pub fn subkey(&self, subkey: Subkey) -> SigningKey {
        SigningKey::from_bytes(&expand(&self.prk, &subkey.info()))
    }

    /// The keys of my encrypted backup (restore servers from the network).
    /// None of them is the identity key or derived from it, so the backup's
    /// holders can't link it to my ID.
    pub fn backup_keys(&self) -> BackupKeys {
        let write = SigningKey::from_bytes(&expand(&self.prk, INFO_BACKUP_WRITE));
        let tag = backup_tag(&write.verifying_key().to_bytes());
        BackupKeys { enc: expand(&self.prk, INFO_BACKUP_KEY), write, tag }
    }
}

/// Where a backup lives: `derive_key("mvs/backup-tag/v1", w_pub)`. A holder
/// stores a blob under `tag` only if it is signed by the write key whose
/// public key gives `tag`.
pub fn backup_tag(write_public: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key(BACKUP_TAG_CONTEXT, write_public)
}

/// The backup's keys: `enc` encrypts it (XChaCha20-Poly1305), `write`
/// authorises every version holders store, `tag` is where it lives.
/// Secret parts are zeroized on drop; `Debug` shows only the tag.
pub struct BackupKeys {
    enc: Zeroizing<[u8; KEY_LEN]>,
    write: SigningKey,
    tag: [u8; 32],
}

impl BackupKeys {
    /// The backup's encryption key. Do not copy it anywhere else.
    pub fn enc_key(&self) -> &[u8; KEY_LEN] {
        &self.enc
    }

    /// Signs each version of the backup; never the identity key.
    pub fn write_key(&self) -> &SigningKey {
        &self.write
    }

    pub fn tag(&self) -> [u8; 32] {
        self.tag
    }
}

impl fmt::Debug for BackupKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupKeys").field("tag", &self.tag).finish_non_exhaustive()
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity").field("public_key", &self.public_key()).finish_non_exhaustive()
    }
}

/// 32-byte symmetric key for local storage. Zeroized on drop.
pub struct StorageKey(Zeroizing<[u8; KEY_LEN]>);

impl StorageKey {
    /// The raw key, for the storage cipher. Do not copy it anywhere else.
    pub fn expose_secret(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for StorageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StorageKey([redacted])")
    }
}

fn expand(prk: &[u8; KEY_LEN], info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hkdf = Hkdf::<Sha256>::from_prk(prk).expect("PRK is one hash long");
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    hkdf.expand(info, &mut *out).expect("32 bytes is a valid HKDF output length");
    out
}
