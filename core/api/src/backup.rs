//! Restore servers from the network, phase A (design note
//! `docs/design/restore-servers-from-network.md`, #291): my server list,
//! sealed with the backup key from my words and signed with the backup
//! write key (`identity::BackupKeys`, #292), so a restore on a clean
//! computer finds my servers again.
//!
//! Wire format (v1), all fixed-size fields first:
//!
//! ```text
//! "mvsb1" ‖ tag[32] ‖ w_pub[32] ‖ version u64le ‖ nonce[24] ‖ ciphertext ‖ sig[64]
//! ```
//!
//! - `tag = derive_key("mvs/backup-tag/v1", w_pub)` (holders recompute it);
//! - the ciphertext is XChaCha20-Poly1305 under the backup key, AAD
//!   `"mvs/backup/v1" ‖ tag ‖ version`, over the plaintext padded to a
//!   multiple of 4 KiB (a u32le length, the JSON, zeros);
//! - `sig` is Ed25519 by `w` over everything before it. No identity key
//!   appears anywhere.
//!
//! Holders keep the highest valid version per tag ([`check`]); readers
//! verify everything again and treat every answer as untrusted ([`open`]).

use std::collections::BTreeMap;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use identity::BackupKeys;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAGIC: &[u8; 5] = b"mvsb1";
const AAD: &[u8] = b"mvs/backup/v1";
const HEADER: usize = 5 + 32 + 32 + 8 + 24;
const SIG: usize = 64;
/// Padding step: the size says how many servers only to 4 KiB.
pub const BACKUP_PAD: usize = 4096;
/// The largest blob a holder takes (design §2).
pub const BACKUP_MAX: usize = 64 * 1024;

/// One server in my backup. `gone` is a tombstone (left, kicked or
/// closed): it beats a membership with an equal or earlier `joined_at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The server's sync secret (GAP-4), hex: only the backup key opens it.
    pub secret: String,
    pub joined_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone_at: Option<u64>,
}

impl Entry {
    pub fn live(&self) -> bool {
        self.gone_at.is_none_or(|gone| gone < self.joined_at)
    }
}

/// The sealed content: server ID (hex) → entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plain {
    pub version: u64,
    pub servers: BTreeMap<String, Entry>,
}

impl Plain {
    /// Merges `other` in (design §6): the union of both, so neither device
    /// erases the other's joins; per server, the later `joined_at` and the
    /// later tombstone win (`live` then compares them).
    pub fn merge(&mut self, other: &Plain) {
        self.version = self.version.max(other.version);
        for (id, theirs) in &other.servers {
            let mine = self.servers.entry(id.clone()).or_insert_with(|| theirs.clone());
            if theirs.joined_at > mine.joined_at {
                mine.joined_at = theirs.joined_at;
                mine.secret = theirs.secret.clone();
            }
            mine.gone_at = mine.gone_at.max(theirs.gone_at);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BackupError {
    /// Not a v1 blob, the wrong size, or a tag that isn't its key's.
    Malformed,
    /// For another account: not my tag or my write key.
    NotMine,
    /// The signature or the encryption doesn't verify (tampered).
    Tampered,
    TooLarge,
}

/// Seals `plain` (its `version` is the one written outside too).
pub fn seal(keys: &BackupKeys, plain: &Plain) -> Result<Vec<u8>, BackupError> {
    let json = Zeroizing::new(serde_json::to_vec(plain).expect("plain struct"));
    let len = u32::try_from(json.len()).map_err(|_| BackupError::TooLarge)?;
    let padded_len = (4 + json.len()).div_ceil(BACKUP_PAD) * BACKUP_PAD;
    if HEADER + padded_len + 16 + SIG > BACKUP_MAX {
        return Err(BackupError::TooLarge);
    }
    let mut padded = Zeroizing::new(vec![0u8; padded_len]);
    padded[..4].copy_from_slice(&len.to_le_bytes());
    padded[4..4 + json.len()].copy_from_slice(&json);

    let tag = keys.tag();
    let w_pub = keys.write_key().verifying_key().to_bytes();
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let aad = [AAD, &tag, &plain.version.to_le_bytes()].concat();
    let sealed = XChaCha20Poly1305::new(keys.enc_key().into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: &padded, aad: &aad })
        .map_err(|_| BackupError::TooLarge)?;
    let mut blob = Vec::with_capacity(HEADER + sealed.len() + SIG);
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&tag);
    blob.extend_from_slice(&w_pub);
    blob.extend_from_slice(&plain.version.to_le_bytes());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&sealed);
    let sig = keys.write_key().sign(&blob);
    blob.extend_from_slice(&sig.to_bytes());
    Ok(blob)
}

/// What a holder checks before storing (R2, R4), with no key: the size,
/// the format, `tag == derive_key(w_pub)` and the signature by `w`.
/// Returns the tag and version.
pub fn check(blob: &[u8]) -> Result<([u8; 32], u64), BackupError> {
    if blob.len() > BACKUP_MAX {
        return Err(BackupError::TooLarge);
    }
    if blob.len() < HEADER + 16 + SIG || &blob[..5] != MAGIC {
        return Err(BackupError::Malformed);
    }
    let tag: [u8; 32] = blob[5..37].try_into().expect("32 bytes");
    let w_pub: [u8; 32] = blob[37..69].try_into().expect("32 bytes");
    let version = u64::from_le_bytes(blob[69..77].try_into().expect("8 bytes"));
    if identity::backup_tag(&w_pub) != tag {
        return Err(BackupError::Malformed);
    }
    let (signed, sig) = blob.split_at(blob.len() - SIG);
    let key = VerifyingKey::from_bytes(&w_pub).map_err(|_| BackupError::Malformed)?;
    let sig = Signature::from_bytes(sig.try_into().expect("64 bytes"));
    // Strict: no weak keys or malleable signatures (Security Lead).
    key.verify_strict(signed, &sig).map_err(|_| BackupError::Tampered)?;
    Ok((tag, version))
}

/// Opens one of my backups: everything [`check`] does, then my tag and
/// write key, the AEAD, the padding and the inner version.
pub fn open(keys: &BackupKeys, blob: &[u8]) -> Result<Plain, BackupError> {
    let (tag, version) = check(blob)?;
    if tag != keys.tag() || blob[37..69] != keys.write_key().verifying_key().to_bytes() {
        return Err(BackupError::NotMine);
    }
    let nonce = &blob[69 + 8..HEADER];
    let sealed = &blob[HEADER..blob.len() - SIG];
    let aad = [AAD, &tag, &version.to_le_bytes()].concat();
    let padded = Zeroizing::new(
        XChaCha20Poly1305::new(keys.enc_key().into())
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &aad })
            .map_err(|_| BackupError::Tampered)?,
    );
    let len = u32::from_le_bytes(padded[..4].try_into().expect("4 bytes")) as usize;
    let json = padded.get(4..4 + len).ok_or(BackupError::Tampered)?;
    let plain: Plain = serde_json::from_slice(json).map_err(|_| BackupError::Tampered)?;
    if plain.version != version {
        return Err(BackupError::Tampered);
    }
    Ok(plain)
}

/// The newest of my backups among untrusted answers; the rest are ignored.
pub fn newest(keys: &BackupKeys, answers: &[Vec<u8>]) -> Option<Plain> {
    answers.iter().filter_map(|blob| open(keys, blob).ok()).max_by_key(|plain| plain.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(words: &str) -> BackupKeys {
        identity::RecoveryPhrase::parse(words).unwrap().identity().backup_keys()
    }

    const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
    const ZOO: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

    fn plain(n: usize) -> Plain {
        let servers = (0..n)
            .map(|i| {
                let entry = Entry { secret: hex::encode([i as u8; 32]), joined_at: i as u64, gone_at: None };
                (hex::encode([i as u8; 32]), entry)
            })
            .collect();
        Plain { version: 7, servers }
    }

    #[test]
    fn a_backup_round_trips_padded_and_checkable_without_the_key() {
        let mine = keys(LEGAL);
        let blob = seal(&mine, &plain(3)).unwrap();
        assert_eq!(open(&mine, &blob).unwrap(), plain(3));
        assert_eq!(check(&blob).unwrap(), (mine.tag(), 7));
        // Padded: 1 and 3 servers give the same size.
        assert_eq!(seal(&mine, &plain(1)).unwrap().len(), blob.len());
        assert_eq!((blob.len() - HEADER - 16 - SIG) % BACKUP_PAD, 0);
        // No identity key anywhere in it.
        let me = identity::RecoveryPhrase::parse(LEGAL).unwrap().identity().public_key();
        assert!(!blob.windows(32).any(|w| w == me.as_bytes()));
    }

    #[test]
    fn a_tampered_backup_is_refused() {
        let mine = keys(LEGAL);
        let blob = seal(&mine, &plain(2)).unwrap();
        for at in [6, 40, 70, 80, HEADER + 5, blob.len() - 1] {
            let mut bad = blob.clone();
            bad[at] ^= 1;
            assert!(open(&mine, &bad).is_err(), "byte {at} changed, still opened");
        }
        // Cut short: not a backup at all.
        assert_eq!(open(&mine, &blob[..10]), Err(BackupError::Malformed));
        assert_eq!(check(&vec![0u8; BACKUP_MAX + 1]), Err(BackupError::TooLarge));
    }

    #[test]
    fn another_accounts_backup_is_refused() {
        let theirs = seal(&keys(ZOO), &plain(2)).unwrap();
        assert!(check(&theirs).is_ok(), "a holder takes it");
        assert_eq!(open(&keys(LEGAL), &theirs), Err(BackupError::NotMine));
        // Their blob re-signed under my tag doesn't open either: my key
        // seals and signs only my own.
        assert!(newest(&keys(LEGAL), &[theirs]).is_none());
    }

    #[test]
    fn newest_takes_the_highest_valid_version_and_merge_keeps_both_devices() {
        let mine = keys(LEGAL);
        let mut old = plain(1);
        old.version = 3;
        let new = plain(2);
        let answers = [seal(&mine, &old).unwrap(), b"junk".to_vec(), seal(&mine, &new).unwrap()];
        assert_eq!(newest(&mine, &answers).unwrap().version, 7);

        // Two devices: the union; a tombstone beats an earlier join, a
        // later rejoin beats the tombstone.
        let mut a = Plain::default();
        a.servers.insert("s1".into(), Entry { secret: "x".into(), joined_at: 1, gone_at: None });
        let mut b = Plain { version: 9, ..Plain::default() };
        b.servers.insert("s2".into(), Entry { secret: "y".into(), joined_at: 2, gone_at: None });
        b.servers.insert("s1".into(), Entry { secret: "x".into(), joined_at: 1, gone_at: Some(5) });
        a.merge(&b);
        assert_eq!(a.version, 9);
        assert!(!a.servers["s1"].live() && a.servers["s2"].live());
        let mut c = Plain::default();
        c.servers.insert("s1".into(), Entry { secret: "z".into(), joined_at: 6, gone_at: None });
        a.merge(&c);
        assert!(a.servers["s1"].live());
        assert_eq!(a.servers["s1"].secret, "z");
    }
}
