//! The stored form of the identity secret, plain or behind an app lock.
//!
//! Layout, version 1:
//!
//! ```text
//! plain:  01 00 | entropy (16)
//! locked: 01 01 | m_kib (u32 LE) | t (u32 LE) | p (u32 LE) | salt (16)
//!               | nonce (24) | XChaCha20-Poly1305(entropy) (16 + 16 tag)
//! ```
//!
//! The AEAD key is Argon2id(passphrase, salt, m, t, p). Everything before the
//! nonce is the associated data, so tampering with the parameters fails.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

const VERSION: u8 = 1;
const KIND_PLAIN: u8 = 0;
const KIND_LOCKED: u8 = 1;
const ENTROPY_LEN: usize = 16;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 2 + 12 + SALT_LEN;
const LOCKED_LEN: usize = HEADER_LEN + NONCE_LEN + ENTROPY_LEN + TAG_LEN;

/// Upper bounds when reading a stored blob, so a corrupted entry cannot make
/// unlocking take forever or exhaust memory.
const MAX_M_KIB: u32 = 256 * 1024; // 256 MiB
const MAX_T: u32 = 10;
const MAX_P: u32 = 8;

/// Argon2id cost for the app lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Passes.
    pub t: u32,
    /// Lanes.
    pub p: u32,
}

impl Default for LockParams {
    /// RFC 9106 section 4, second recommended option: 64 MiB, 3 passes,
    /// 4 lanes.
    fn default() -> LockParams {
        LockParams { m_kib: 64 * 1024, t: 3, p: 4 }
    }
}

impl LockParams {
    /// The lowest cost a new lock may use: OWASP's Argon2id minimum (19 MiB,
    /// 2 passes, 1 lane).
    pub const MIN: LockParams = LockParams { m_kib: 19 * 1024, t: 2, p: 1 };

    fn at_least_min(self) -> bool {
        self.m_kib >= LockParams::MIN.m_kib
            && self.t >= LockParams::MIN.t
            && self.p >= LockParams::MIN.p
    }

    fn within_max(self) -> bool {
        self.m_kib <= MAX_M_KIB && self.t <= MAX_T && self.p <= MAX_P
    }
}

pub(crate) enum Decoded {
    Plain(Zeroizing<[u8; ENTROPY_LEN]>),
    Locked,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockError {
    Corrupt,
    WrongPassphrase,
    WeakParams,
}

pub(crate) fn encode_plain(entropy: &[u8; ENTROPY_LEN]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(2 + ENTROPY_LEN));
    out.extend_from_slice(&[VERSION, KIND_PLAIN]);
    out.extend_from_slice(entropy);
    out
}

pub(crate) fn decode(blob: &[u8]) -> Result<Decoded, LockError> {
    match blob {
        [VERSION, KIND_PLAIN, rest @ ..] if rest.len() == ENTROPY_LEN => {
            let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
            entropy.copy_from_slice(rest);
            Ok(Decoded::Plain(entropy))
        }
        [VERSION, KIND_LOCKED, ..] if blob.len() == LOCKED_LEN => Ok(Decoded::Locked),
        _ => Err(LockError::Corrupt),
    }
}

pub(crate) fn seal(
    entropy: &[u8; ENTROPY_LEN],
    passphrase: &str,
    params: LockParams,
) -> Result<Vec<u8>, LockError> {
    // Also refuse a cost above what `open` accepts, or the lock could never
    // be opened.
    if !params.at_least_min() || !params.within_max() {
        return Err(LockError::WeakParams);
    }
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);

    let mut out = Vec::with_capacity(LOCKED_LEN);
    out.extend_from_slice(&[VERSION, KIND_LOCKED]);
    out.extend_from_slice(&params.m_kib.to_le_bytes());
    out.extend_from_slice(&params.t.to_le_bytes());
    out.extend_from_slice(&params.p.to_le_bytes());
    out.extend_from_slice(&salt);

    let key = kek(passphrase, &salt, params)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let sealed = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: entropy, aad: &out })
        .map_err(|_| LockError::Corrupt)?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub(crate) fn open(
    blob: &[u8],
    passphrase: &str,
) -> Result<Zeroizing<[u8; ENTROPY_LEN]>, LockError> {
    if blob.len() != LOCKED_LEN || blob[..2] != [VERSION, KIND_LOCKED] {
        return Err(LockError::Corrupt);
    }
    let u32_at = |i: usize| u32::from_le_bytes(blob[i..i + 4].try_into().expect("4 bytes"));
    let params = LockParams { m_kib: u32_at(2), t: u32_at(6), p: u32_at(10) };
    if !params.within_max() {
        return Err(LockError::Corrupt);
    }
    let (header, rest) = blob.split_at(HEADER_LEN);
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let salt = &header[14..];

    let key = kek(passphrase, salt, params)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let plain = Zeroizing::new(
        cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: header })
            .map_err(|_| LockError::WrongPassphrase)?,
    );
    let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
    entropy.copy_from_slice(&plain);
    Ok(entropy)
}

fn kek(
    passphrase: &str,
    salt: &[u8],
    params: LockParams,
) -> Result<Zeroizing<[u8; 32]>, LockError> {
    let params =
        Params::new(params.m_kib, params.t, params.p, Some(32)).map_err(|_| LockError::Corrupt)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    // NFC, so the same text typed on another OS or keyboard gives the same
    // key (Hebrew niqqud and geresh can arrive composed or decomposed).
    let passphrase: Zeroizing<String> = Zeroizing::new(passphrase.nfc().collect());
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut *key)
        .map_err(|_| LockError::Corrupt)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the lock KDF: Argon2id v0x13 with the passphrase NFC-normalised.
    /// A dependency bump must not change it, or existing app locks stop
    /// opening. Small cost so the test is fast; the parameters are part of
    /// the input, not the pin.
    #[test]
    fn lock_kdf_is_pinned() {
        let params = LockParams { m_kib: 1024, t: 2, p: 1 };
        let key = kek("סיסמה correct horse", b"0123456789abcdef", params).unwrap();
        assert_eq!(hex::encode(*key), GOLDEN_KEK);
    }

    const GOLDEN_KEK: &str = "3e7f80dfa6fad77c2a2013f1babcf80aa795ef9a2a3b8d2048d1ec9eccdf659f";
}
