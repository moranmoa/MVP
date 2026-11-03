//! The signed release manifest and its trust root.
//!
//! Wire format (JSON):
//!
//! ```json
//! { "signed": "<manifest body as a JSON string>",
//!   "signatures": [ { "key": "<hex public key>", "sig": "<hex signature>" } ] }
//! ```
//!
//! Signatures cover the exact bytes of `signed` (behind a domain-separation
//! prefix), so verification never depends on re-serialising JSON, and the
//! body is only parsed after enough signatures check out.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature as EdSignature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{Error, Version};

/// The only body format this code understands.
pub const FORMAT: u32 = 1;

/// Prepended to the body before signing, so a root key's signature on a
/// manifest can never be mistaken for a signature on anything else.
pub const SIGNING_CONTEXT: &[u8] = b"p2p-chat/update-manifest/v1\0";

/// Peers hand us manifests; refuse anything absurdly large before parsing.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// What one release contains. Written by the release engineer, then signed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBody {
    pub format: u32,
    /// Update channel, e.g. `stable` or `beta`.
    pub channel: String,
    pub version: Version,
    /// Unix seconds. Informational, but must precede `expires_at`.
    pub issued_at: u64,
    /// Unix seconds. Clients refuse the manifest from this moment on, so an
    /// attacker cannot freeze a user on an old release by replaying it.
    pub expires_at: u64,
    /// Keyed by platform, e.g. `windows-x86_64`.
    pub artifacts: BTreeMap<String, Artifact>,
    /// Versions that must never be installed or rolled back to.
    #[serde(default)]
    pub revoked: BTreeSet<Version>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Lowercase hex SHA-256 of the artifact file.
    pub sha256: String,
    pub size: u64,
}

impl ManifestBody {
    /// Internal consistency checks, independent of time and client state.
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |why: &str| Err(Error::Malformed(why.to_string()));
        if self.format != FORMAT {
            return Err(Error::Malformed(format!("unsupported format {}", self.format)));
        }
        if self.channel.is_empty() {
            return bad("empty channel");
        }
        if self.expires_at <= self.issued_at {
            return bad("expires_at must be after issued_at");
        }
        if self.artifacts.is_empty() {
            return bad("no artifacts");
        }
        for (platform, a) in &self.artifacts {
            let hex_ok = a.sha256.len() == 64
                && a.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if platform.is_empty() || !hex_ok {
                return Err(Error::Malformed(format!("bad artifact entry {platform:?}")));
            }
        }
        if self.revoked.contains(&self.version) {
            return bad("manifest revokes its own version");
        }
        Ok(())
    }
}

/// The set of root public keys baked into the app, plus how many of them
/// must sign. Closed alpha: one key, threshold 1. From the open alpha on:
/// three hardware keys, threshold 2.
#[derive(Clone, Debug)]
pub struct TrustRoot {
    keys: Vec<VerifyingKey>,
    threshold: usize,
}

/// On-disk / embedded form of a [`TrustRoot`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRootFile {
    pub threshold: usize,
    /// Hex Ed25519 public keys.
    pub keys: Vec<String>,
}

impl TrustRoot {
    pub fn new(keys: Vec<VerifyingKey>, threshold: usize) -> Result<Self, Error> {
        let bad = |why: &str| Err(Error::InvalidTrustRoot(why.to_string()));
        if threshold == 0 {
            return bad("threshold must be at least 1");
        }
        if threshold > keys.len() {
            return bad("threshold is larger than the number of keys");
        }
        let distinct: BTreeSet<[u8; 32]> = keys.iter().map(|k| k.to_bytes()).collect();
        if distinct.len() != keys.len() {
            return bad("duplicate key");
        }
        if keys.iter().any(|k| k.is_weak()) {
            return bad("weak key");
        }
        Ok(TrustRoot { keys, threshold })
    }

    /// The closed-alpha configuration.
    pub fn single(key: VerifyingKey) -> Result<Self, Error> {
        TrustRoot::new(vec![key], 1)
    }

    pub fn from_file(file: &TrustRootFile) -> Result<Self, Error> {
        let keys = file
            .keys
            .iter()
            .map(|h| parse_public_key(h).ok_or_else(|| Error::InvalidTrustRoot(format!("bad key {h:?}"))))
            .collect::<Result<Vec<_>, _>>()?;
        TrustRoot::new(keys, file.threshold)
    }

    pub fn to_file(&self) -> TrustRootFile {
        TrustRootFile {
            threshold: self.threshold,
            keys: self.keys.iter().map(|k| hex::encode(k.to_bytes())).collect(),
        }
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    fn index_of(&self, key: &VerifyingKey) -> Option<usize> {
        self.keys.iter().position(|k| k == key)
    }
}

pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEntry {
    /// Hex public key of the signer.
    pub key: String,
    /// Hex Ed25519 signature over `SIGNING_CONTEXT || signed`.
    pub sig: String,
}

/// A manifest as it travels: body bytes plus detached signatures.
/// Nothing in here is trusted until [`SignedManifest::verify`] succeeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    pub signed: String,
    pub signatures: Vec<SignatureEntry>,
}

fn signing_message(signed: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(SIGNING_CONTEXT.len() + signed.len());
    msg.extend_from_slice(SIGNING_CONTEXT);
    msg.extend_from_slice(signed.as_bytes());
    msg
}

impl SignedManifest {
    /// Serialises `body` and returns it unsigned. Refuses invalid bodies so a
    /// release engineer cannot sign something every client would reject.
    pub fn new(body: &ManifestBody) -> Result<Self, Error> {
        body.validate()?;
        let signed = serde_json::to_string(body).map_err(|e| Error::Malformed(e.to_string()))?;
        Ok(SignedManifest { signed, signatures: Vec::new() })
    }

    /// Adds one signature. Each root key holder runs this separately, on
    /// their own machine, for the 2-of-3 setup. Re-signing with the same key
    /// replaces that key's previous signature.
    pub fn sign(&mut self, key: &SigningKey) {
        let sig = key.sign(&signing_message(&self.signed));
        let key_hex = hex::encode(key.verifying_key().to_bytes());
        self.signatures.retain(|s| s.key != key_hex);
        self.signatures.push(SignatureEntry { key: key_hex, sig: hex::encode(sig.to_bytes()) });
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Malformed(format!("larger than {MAX_MANIFEST_BYTES} bytes")));
        }
        serde_json::from_slice(bytes).map_err(|e| Error::Malformed(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest always serialises")
    }

    /// Checks signatures, then format, then expiry. `now` is Unix seconds.
    ///
    /// Signatures by keys outside the trust root, malformed entries and
    /// repeated signatures by one key are ignored: they never count toward
    /// the threshold, and never make an otherwise valid manifest fail.
    pub fn verify(&self, root: &TrustRoot, now: u64) -> Result<VerifiedManifest, Error> {
        let msg = signing_message(&self.signed);
        let mut signers = BTreeSet::new();
        for entry in &self.signatures {
            let Some(key) = parse_public_key(&entry.key) else { continue };
            let Some(index) = root.index_of(&key) else { continue };
            let Some(sig) = hex::decode(&entry.sig).ok().and_then(|b| EdSignature::from_slice(&b).ok()) else {
                continue;
            };
            if key.verify_strict(&msg, &sig).is_ok() {
                signers.insert(index);
            }
        }
        if signers.len() < root.threshold {
            return Err(Error::InsufficientSignatures { valid: signers.len(), required: root.threshold });
        }

        let body: ManifestBody =
            serde_json::from_str(&self.signed).map_err(|e| Error::Malformed(e.to_string()))?;
        body.validate()?;
        if now >= body.expires_at {
            return Err(Error::Expired { expires_at: body.expires_at, now });
        }
        Ok(VerifiedManifest { body })
    }
}

/// A manifest whose signatures, format and expiry have been checked.
/// Only [`SignedManifest::verify`] can make one, so any function taking a
/// `&VerifiedManifest` knows those checks already ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedManifest {
    body: ManifestBody,
}

impl VerifiedManifest {
    pub fn body(&self) -> &ManifestBody {
        &self.body
    }

    pub fn version(&self) -> Version {
        self.body.version
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rand_core::OsRng;

    pub const NOW: u64 = 1_800_000_000;

    pub fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    pub fn body(version: &str) -> ManifestBody {
        let mut artifacts = BTreeMap::new();
        artifacts.insert("linux-x86_64".to_string(), Artifact { sha256: "ab".repeat(32), size: 10 });
        ManifestBody {
            format: FORMAT,
            channel: "stable".to_string(),
            version: version.parse().unwrap(),
            issued_at: NOW - 100,
            expires_at: NOW + 7 * 86_400,
            artifacts,
            revoked: BTreeSet::new(),
        }
    }

    pub fn signed_by(body: &ManifestBody, keys: &[&SigningKey]) -> SignedManifest {
        let mut m = SignedManifest::new(body).unwrap();
        for k in keys {
            m.sign(k);
        }
        m
    }

    fn root(keys: &[&SigningKey], threshold: usize) -> TrustRoot {
        TrustRoot::new(keys.iter().map(|k| k.verifying_key()).collect(), threshold).unwrap()
    }

    // --- single-key (closed alpha) ---

    #[test]
    fn single_key_manifest_verifies() {
        let k = key();
        let m = signed_by(&body("1.0.0"), &[&k]);
        let v = m.verify(&TrustRoot::single(k.verifying_key()).unwrap(), NOW).unwrap();
        assert_eq!(v.version(), Version::new(1, 0, 0));
    }

    #[test]
    fn unsigned_manifest_is_rejected() {
        let k = key();
        let m = signed_by(&body("1.0.0"), &[]);
        assert_eq!(
            m.verify(&root(&[&k], 1), NOW),
            Err(Error::InsufficientSignatures { valid: 0, required: 1 })
        );
    }

    #[test]
    fn signature_by_untrusted_key_is_rejected() {
        let trusted = key();
        let attacker = key();
        let m = signed_by(&body("1.0.0"), &[&attacker]);
        assert!(matches!(m.verify(&root(&[&trusted], 1), NOW), Err(Error::InsufficientSignatures { .. })));
    }

    #[test]
    fn tampered_body_is_rejected() {
        let k = key();
        let mut m = signed_by(&body("1.0.0"), &[&k]);
        m.signed = m.signed.replace("1.0.0", "9.0.0");
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::InsufficientSignatures { .. })));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let k = key();
        let mut m = signed_by(&body("1.0.0"), &[&k]);
        let mut sig = hex::decode(&m.signatures[0].sig).unwrap();
        sig[0] ^= 1;
        m.signatures[0].sig = hex::encode(sig);
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::InsufficientSignatures { .. })));
    }

    #[test]
    fn signature_without_domain_prefix_is_rejected() {
        let k = key();
        let mut m = signed_by(&body("1.0.0"), &[]);
        let raw = k.sign(m.signed.as_bytes());
        m.signatures.push(SignatureEntry {
            key: hex::encode(k.verifying_key().to_bytes()),
            sig: hex::encode(raw.to_bytes()),
        });
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::InsufficientSignatures { .. })));
    }

    #[test]
    fn garbage_signature_entries_are_ignored_not_fatal() {
        let k = key();
        let mut m = signed_by(&body("1.0.0"), &[&k]);
        m.signatures.insert(0, SignatureEntry { key: "zz".into(), sig: "zz".into() });
        m.signatures.push(SignatureEntry { key: hex::encode(k.verifying_key().to_bytes()), sig: "00".into() });
        assert!(m.verify(&root(&[&k], 1), NOW).is_ok());
    }

    // --- 2-of-3 threshold (open alpha onward) ---

    #[test]
    fn two_of_three_accepts_any_two() {
        let (a, b, c) = (key(), key(), key());
        let r = root(&[&a, &b, &c], 2);
        for pair in [[&a, &b], [&a, &c], [&b, &c]] {
            assert!(signed_by(&body("1.0.0"), &pair).verify(&r, NOW).is_ok());
        }
        assert!(signed_by(&body("1.0.0"), &[&a, &b, &c]).verify(&r, NOW).is_ok());
    }

    #[test]
    fn two_of_three_rejects_one() {
        let (a, b, c) = (key(), key(), key());
        let r = root(&[&a, &b, &c], 2);
        assert_eq!(
            signed_by(&body("1.0.0"), &[&b]).verify(&r, NOW),
            Err(Error::InsufficientSignatures { valid: 1, required: 2 })
        );
    }

    #[test]
    fn same_key_twice_counts_once() {
        let (a, b, c) = (key(), key(), key());
        let r = root(&[&a, &b, &c], 2);
        let mut m = signed_by(&body("1.0.0"), &[&a]);
        let dup = m.signatures[0].clone();
        m.signatures.push(dup);
        assert_eq!(m.verify(&r, NOW), Err(Error::InsufficientSignatures { valid: 1, required: 2 }));
    }

    #[test]
    fn re_signing_replaces_instead_of_duplicating() {
        let a = key();
        let mut m = signed_by(&body("1.0.0"), &[&a]);
        m.sign(&a);
        assert_eq!(m.signatures.len(), 1);
    }

    #[test]
    fn trusted_plus_untrusted_signature_is_not_two() {
        let (a, b, c, outsider) = (key(), key(), key(), key());
        let r = root(&[&a, &b, &c], 2);
        let m = signed_by(&body("1.0.0"), &[&a, &outsider]);
        assert_eq!(m.verify(&r, NOW), Err(Error::InsufficientSignatures { valid: 1, required: 2 }));
    }

    #[test]
    fn trust_root_rejects_bad_configurations() {
        let (a, b) = (key(), key());
        let ka = a.verifying_key();
        assert!(TrustRoot::new(vec![ka], 0).is_err());
        assert!(TrustRoot::new(vec![ka], 2).is_err());
        assert!(TrustRoot::new(vec![ka, ka], 1).is_err());
        assert!(TrustRoot::new(vec![], 1).is_err());
        assert!(TrustRoot::new(vec![ka, b.verifying_key()], 2).is_ok());
    }

    #[test]
    fn trust_root_file_round_trips() {
        let (a, b, c) = (key(), key(), key());
        let r = root(&[&a, &b, &c], 2);
        let back = TrustRoot::from_file(&r.to_file()).unwrap();
        assert!(signed_by(&body("1.0.0"), &[&a, &c]).verify(&back, NOW).is_ok());
    }

    // --- expiry ---

    #[test]
    fn expired_manifest_is_rejected() {
        let k = key();
        let b = body("1.0.0");
        let m = signed_by(&b, &[&k]);
        let r = root(&[&k], 1);
        assert!(m.verify(&r, b.expires_at - 1).is_ok());
        assert_eq!(m.verify(&r, b.expires_at), Err(Error::Expired { expires_at: b.expires_at, now: b.expires_at }));
        assert!(m.verify(&r, b.expires_at + 1_000_000).is_err());
    }

    // --- body validation ---

    #[test]
    fn invalid_bodies_cannot_be_signed() {
        let mut b = body("1.0.0");
        b.format = 2;
        assert!(SignedManifest::new(&b).is_err());

        let mut b = body("1.0.0");
        b.expires_at = b.issued_at;
        assert!(SignedManifest::new(&b).is_err());

        let mut b = body("1.0.0");
        b.artifacts.clear();
        assert!(SignedManifest::new(&b).is_err());

        let mut b = body("1.0.0");
        b.artifacts.get_mut("linux-x86_64").unwrap().sha256 = "AB".repeat(32);
        assert!(SignedManifest::new(&b).is_err());

        let mut b = body("1.0.0");
        b.revoked.insert(b.version);
        assert!(SignedManifest::new(&b).is_err());
    }

    #[test]
    fn signed_but_invalid_body_is_still_rejected_by_clients() {
        // A buggy signing tool must not be able to push an inconsistent body.
        let k = key();
        let mut b = body("1.0.0");
        b.revoked.insert(b.version);
        let mut m = SignedManifest { signed: serde_json::to_string(&b).unwrap(), signatures: vec![] };
        m.sign(&k);
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::Malformed(_))));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let k = key();
        let json = serde_json::to_string(&body("1.0.0")).unwrap();
        let json = json.replacen('{', "{\"surprise\":1,", 1);
        let mut m = SignedManifest { signed: json, signatures: vec![] };
        m.sign(&k);
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::Malformed(_))));
    }

    #[test]
    fn json_round_trip_keeps_signatures_valid() {
        let k = key();
        let m = signed_by(&body("1.0.0"), &[&k]);
        let back = SignedManifest::from_json(m.to_json().as_bytes()).unwrap();
        assert!(back.verify(&root(&[&k], 1), NOW).is_ok());
    }

    #[test]
    fn oversized_input_is_refused_before_parsing() {
        let big = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(matches!(SignedManifest::from_json(&big), Err(Error::Malformed(_))));
    }
}
