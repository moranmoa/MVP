//! The signed release manifest.
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
use std::io::Read;

use ed25519_dalek::{Signature as EdSignature, Signer, SigningKey};
use serde::{Deserialize, Serialize};

use crate::root::parse_public_key;
use crate::{Error, TrustRoot, Version};

/// The only body format this code understands.
pub const FORMAT: u32 = 1;

/// Prepended to the body before signing, so a root key's signature on a
/// manifest can never be mistaken for a signature on anything else.
pub const SIGNING_CONTEXT: &[u8] = b"p2p-chat/update-manifest/v1\0";

/// Peers hand us manifests; refuse anything absurdly large before parsing.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// More signature entries than any honest root needs (3 keys today). Caps
/// the Ed25519 work a hostile peer can make us do per manifest.
pub const MAX_SIGNATURES: usize = 8;

/// Longest allowed gap between `issued_at` and `expires_at`. A manifest
/// that stayed valid for years would defeat freeze protection.
pub const MAX_VALIDITY_SECS: u64 = 90 * 86_400;

/// What one release contains. Written by the release engineer, then signed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBody {
    pub format: u32,
    /// Update channel, e.g. `stable` or `beta`.
    pub channel: String,
    pub version: Version,
    /// Unix seconds. Clients use the highest `issued_at` they have accepted
    /// as a lower bound on the current time (see `ClientPolicy`).
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

/// Channel and platform names end up in file names and log lines, so they
/// are restricted to a boring alphabet.
pub fn is_valid_name(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

impl ManifestBody {
    /// Internal consistency checks, independent of time and client state.
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |why: &str| Err(Error::Malformed(why.to_string()));
        if self.format != FORMAT {
            return Err(Error::Malformed(format!("unsupported format {}", self.format)));
        }
        if !is_valid_name(&self.channel) {
            return bad("bad channel name");
        }
        if self.expires_at <= self.issued_at {
            return bad("expires_at must be after issued_at");
        }
        if self.expires_at - self.issued_at > MAX_VALIDITY_SECS {
            return bad("validity period longer than MAX_VALIDITY_SECS");
        }
        if self.artifacts.is_empty() {
            return bad("no artifacts");
        }
        for (platform, a) in &self.artifacts {
            let hex_ok = a.sha256.len() == 64
                && a.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if !is_valid_name(platform) || !hex_ok {
                return Err(Error::Malformed(format!("bad artifact entry {platform:?}")));
            }
        }
        if self.revoked.contains(&self.version) {
            return bad("manifest revokes its own version");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEntry {
    /// Hex public key of the signer.
    pub key: String,
    /// Hex Ed25519 signature over `context || signed`.
    pub sig: String,
}

fn signing_message(context: &[u8], signed: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(context.len() + signed.len());
    msg.extend_from_slice(context);
    msg.extend_from_slice(signed.as_bytes());
    msg
}

/// Adds or replaces `key`'s signature over `context || signed`.
pub(crate) fn add_signature(context: &[u8], signed: &str, sigs: &mut Vec<SignatureEntry>, key: &SigningKey) {
    let sig = key.sign(&signing_message(context, signed));
    let key_hex = hex::encode(key.verifying_key().to_bytes());
    sigs.retain(|s| s.key != key_hex);
    sigs.push(SignatureEntry { key: key_hex, sig: hex::encode(sig.to_bytes()) });
}

/// How many distinct keys of `root` validly signed. Entries by unknown keys,
/// malformed entries and repeats never count and never cause an error;
/// only an oversized list does.
pub(crate) fn valid_signers(
    context: &[u8],
    signed: &str,
    sigs: &[SignatureEntry],
    root: &TrustRoot,
) -> Result<usize, Error> {
    if sigs.len() > MAX_SIGNATURES {
        return Err(Error::Malformed(format!("more than {MAX_SIGNATURES} signatures")));
    }
    let msg = signing_message(context, signed);
    let mut signers = BTreeSet::new();
    for entry in sigs {
        let Some(key) = parse_public_key(&entry.key) else { continue };
        let Some(index) = root.index_of(&key) else { continue };
        if signers.contains(&index) {
            continue;
        }
        let Some(sig) = hex::decode(&entry.sig).ok().and_then(|b| EdSignature::from_slice(&b).ok()) else {
            continue;
        };
        if key.verify_strict(&msg, &sig).is_ok() {
            signers.insert(index);
        }
    }
    Ok(signers.len())
}

/// A manifest as it travels: body bytes plus detached signatures.
/// Nothing in here is trusted until [`SignedManifest::verify`] succeeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    pub signed: String,
    pub signatures: Vec<SignatureEntry>,
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
        add_signature(SIGNING_CONTEXT, &self.signed, &mut self.signatures, key);
    }

    /// The body, parsed and validated but **not** signature-checked. For a
    /// co-signer to read before adding their signature; never for clients.
    pub fn body_unverified(&self) -> Result<ManifestBody, Error> {
        let body: ManifestBody =
            serde_json::from_str(&self.signed).map_err(|e| Error::Malformed(e.to_string()))?;
        body.validate()?;
        Ok(body)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Malformed(format!("larger than {MAX_MANIFEST_BYTES} bytes")));
        }
        serde_json::from_slice(bytes).map_err(|e| Error::Malformed(e.to_string()))
    }

    /// Reads at most `MAX_MANIFEST_BYTES + 1` bytes, so a peer streaming an
    /// endless manifest cannot exhaust memory.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, Error> {
        let mut buf = Vec::new();
        reader.take(MAX_MANIFEST_BYTES as u64 + 1).read_to_end(&mut buf)?;
        SignedManifest::from_json(&buf)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest always serialises")
    }

    /// Checks signatures, then format, then expiry. `now` is Unix seconds.
    /// Clients should go through `ClientPolicy::accept` afterwards, which
    /// also enforces expiry against the signed time floor.
    pub fn verify(&self, root: &TrustRoot, now: u64) -> Result<VerifiedManifest, Error> {
        let valid = valid_signers(SIGNING_CONTEXT, &self.signed, &self.signatures, root)?;
        if valid < root.threshold() {
            return Err(Error::InsufficientSignatures { valid, required: root.threshold() });
        }
        let body = self.body_unverified()?;
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

    #[test]
    fn too_many_signature_entries_are_refused() {
        let k = key();
        let mut m = signed_by(&body("1.0.0"), &[&k]);
        let filler = SignatureEntry { key: hex::encode(k.verifying_key().to_bytes()), sig: "00".repeat(64) };
        m.signatures.extend(std::iter::repeat(filler).take(MAX_SIGNATURES));
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::Malformed(_))));
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
    fn validity_period_is_capped() {
        let mut b = body("1.0.0");
        b.expires_at = b.issued_at + MAX_VALIDITY_SECS;
        assert!(SignedManifest::new(&b).is_ok());
        b.expires_at += 1;
        assert!(SignedManifest::new(&b).is_err());
    }

    #[test]
    fn path_like_names_are_rejected() {
        for bad in ["../../evil", "win\\x", "Linux", "", "a/b", "x".repeat(33).as_str(), "nul\0"] {
            let mut b = body("1.0.0");
            let entry = b.artifacts.remove("linux-x86_64").unwrap();
            b.artifacts.insert(bad.to_string(), entry);
            assert!(SignedManifest::new(&b).is_err(), "platform {bad:?}");
            let mut b = body("1.0.0");
            b.channel = bad.to_string();
            assert!(SignedManifest::new(&b).is_err(), "channel {bad:?}");
        }
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
        // An endless stream is cut off after MAX_MANIFEST_BYTES + 1 bytes.
        assert!(matches!(SignedManifest::from_reader(std::io::repeat(b' ')), Err(Error::Malformed(_))));
    }

    #[test]
    fn deeply_nested_json_is_refused_not_a_stack_overflow() {
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        assert!(SignedManifest::from_json(deep.as_bytes()).is_err());
        let k = key();
        let mut m = SignedManifest { signed: deep, signatures: vec![] };
        m.sign(&k);
        assert!(matches!(m.verify(&root(&[&k], 1), NOW), Err(Error::Malformed(_))));
    }
}
