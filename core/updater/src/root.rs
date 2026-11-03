//! The trust root (which keys may sign releases) and how it is rotated.
//!
//! Rotation, e.g. closed alpha (1 key) → open alpha (2-of-3 hardware keys):
//!
//! 1. Write root version N+1 with the new keys and threshold.
//! 2. Sign it with a threshold of the **current** keys (proves the owner
//!    wants it) and a threshold of the **new** keys (proves the new key
//!    holders exist and can sign, so a typo cannot lock everyone out).
//! 3. Peers hand the chain of root updates around with the manifests.
//!    [`TrustRoot::update_chain`] applies them one version at a time.
//!
//! After a client applies N+1 it no longer trusts the old keys at all
//! (unless they are listed again), so nobody trusts both sets forever.
//! New releases embed the latest root, so fresh installs start there.

use std::collections::BTreeSet;

use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::manifest::{add_signature, valid_signers, SignatureEntry, MAX_MANIFEST_BYTES};
use crate::Error;

/// Domain prefix for root-rotation signatures (distinct from manifests).
pub const ROOT_SIGNING_CONTEXT: &[u8] = b"mvs/trust-root/v1\0";

/// The set of root public keys, plus how many of them must sign.
/// Closed alpha: one key, threshold 1. From the open alpha on: three
/// hardware keys, threshold 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustRoot {
    version: u64,
    keys: Vec<VerifyingKey>,
    threshold: usize,
}

/// On-disk / embedded form of a [`TrustRoot`], and the body of a rotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRootFile {
    /// Starts at 1; each rotation adds exactly 1.
    #[serde(default = "first_version")]
    pub version: u64,
    pub threshold: usize,
    /// Hex Ed25519 public keys.
    pub keys: Vec<String>,
}

fn first_version() -> u64 {
    1
}

impl TrustRoot {
    pub fn new(keys: Vec<VerifyingKey>, threshold: usize) -> Result<Self, Error> {
        TrustRoot::with_version(1, keys, threshold)
    }

    pub fn with_version(version: u64, keys: Vec<VerifyingKey>, threshold: usize) -> Result<Self, Error> {
        let bad = |why: &str| Err(Error::InvalidTrustRoot(why.to_string()));
        if version == 0 {
            return bad("version must be at least 1");
        }
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
        Ok(TrustRoot { version, keys, threshold })
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
        TrustRoot::with_version(file.version, keys, file.threshold)
    }

    pub fn to_file(&self) -> TrustRootFile {
        TrustRootFile {
            version: self.version,
            threshold: self.threshold,
            keys: self.keys.iter().map(|k| hex::encode(k.to_bytes())).collect(),
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub(crate) fn index_of(&self, key: &VerifyingKey) -> Option<usize> {
        self.keys.iter().position(|k| k == key)
    }

    /// Applies one rotation. It must be exactly the next version and be
    /// signed by a threshold of both the current and the new keys.
    pub fn rotate(&self, update: &SignedRoot) -> Result<TrustRoot, Error> {
        let next = update.body()?;
        let expected = self.version.checked_add(1).ok_or(Error::RootVersion { expected: u64::MAX, found: next.version })?;
        if next.version != expected {
            return Err(Error::RootVersion { expected, found: next.version });
        }
        let old = valid_signers(ROOT_SIGNING_CONTEXT, &update.signed, &update.signatures, self)?;
        if old < self.threshold {
            return Err(Error::InsufficientSignatures { valid: old, required: self.threshold });
        }
        let new = TrustRoot::from_file(&next)?;
        let fresh = valid_signers(ROOT_SIGNING_CONTEXT, &update.signed, &update.signatures, &new)?;
        if fresh < new.threshold {
            return Err(Error::InsufficientSignatures { valid: fresh, required: new.threshold });
        }
        Ok(new)
    }

    /// Advances through a set of rotations as far as valid links allow.
    ///
    /// The set comes from peers, so it is never trusted to be clean: junk
    /// entries, forged rotations and padded copies are skipped instead of
    /// aborting (an abort would let any peer freeze clients on a retired
    /// root). Entries at or below the current version are ignored, so peers
    /// can always send the whole chain. A gap simply stops the walk.
    ///
    /// Two *different* rotations to the same version that are both validly
    /// signed can only mean a current-root key leaked. The walk then stops
    /// before that version and reports it in [`RootChain::fork_at`]; the app
    /// should raise a compromise alarm rather than pick one.
    pub fn update_chain(&self, updates: &[SignedRoot]) -> RootChain {
        let updates = &updates[..updates.len().min(MAX_ROOT_UPDATES)];
        let mut root = self.clone();
        let mut fork_at = None;
        loop {
            let Some(target) = root.version.checked_add(1) else { break };
            let mut next: Option<TrustRoot> = None;
            for u in updates {
                // Cheap version check first; signatures only for candidates.
                if u.body().map(|b| b.version) != Ok(target) {
                    continue;
                }
                let Ok(candidate) = root.rotate(u) else { continue };
                match &next {
                    None => next = Some(candidate),
                    Some(seen) if *seen == candidate => {}
                    Some(_) => fork_at = Some(target),
                }
            }
            match next {
                Some(n) if fork_at.is_none() => root = n,
                _ => break,
            }
        }
        let skipped = updates.iter().filter(|u| u.body().map_or(true, |b| b.version > root.version)).count();
        RootChain { root, skipped, fork_at }
    }
}

/// At most this many root updates are looked at per call, capping the
/// signature work a peer can cause. Rotations are rare; 32 is plenty.
pub const MAX_ROOT_UPDATES: usize = 32;

/// The outcome of [`TrustRoot::update_chain`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootChain {
    /// The newest root reached. Use it, even if `skipped > 0`.
    pub root: TrustRoot,
    /// Entries above `root`'s version that could not be applied (junk,
    /// forged, or beyond a gap). Informational.
    pub skipped: usize,
    /// Two different validly signed rotations to this version were seen:
    /// a current-root key has leaked. Alarm; `root` stops just before it.
    pub fork_at: Option<u64>,
}

pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// A signed root rotation. Same envelope as a manifest, different domain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedRoot {
    pub signed: String,
    pub signatures: Vec<SignatureEntry>,
}

impl SignedRoot {
    pub fn new(next: &TrustRootFile) -> Result<Self, Error> {
        TrustRoot::from_file(next)?;
        let signed = serde_json::to_string(next).map_err(|e| Error::Malformed(e.to_string()))?;
        Ok(SignedRoot { signed, signatures: Vec::new() })
    }

    pub fn sign(&mut self, key: &SigningKey) {
        add_signature(ROOT_SIGNING_CONTEXT, &self.signed, &mut self.signatures, key);
    }

    /// The proposed root, *unverified*. For display before co-signing.
    pub fn body(&self) -> Result<TrustRootFile, Error> {
        serde_json::from_str(&self.signed).map_err(|e| Error::Malformed(e.to_string()))
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Malformed(format!("larger than {MAX_MANIFEST_BYTES} bytes")));
        }
        serde_json::from_slice(bytes).map_err(|e| Error::Malformed(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("root update always serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{body, key, signed_by, NOW};

    fn file(version: u64, keys: &[&SigningKey], threshold: usize) -> TrustRootFile {
        TrustRootFile {
            version,
            threshold,
            keys: keys.iter().map(|k| hex::encode(k.verifying_key().to_bytes())).collect(),
        }
    }

    fn rotation(next: &TrustRootFile, signers: &[&SigningKey]) -> SignedRoot {
        let mut r = SignedRoot::new(next).unwrap();
        for k in signers {
            r.sign(k);
        }
        r
    }

    #[test]
    fn trust_root_rejects_bad_configurations() {
        let (a, b) = (key(), key());
        let ka = a.verifying_key();
        assert!(TrustRoot::new(vec![ka], 0).is_err());
        assert!(TrustRoot::new(vec![ka], 2).is_err());
        assert!(TrustRoot::new(vec![ka, ka], 1).is_err());
        assert!(TrustRoot::new(vec![], 1).is_err());
        assert!(TrustRoot::with_version(0, vec![ka], 1).is_err());
        assert!(TrustRoot::new(vec![ka, b.verifying_key()], 2).is_ok());
    }

    #[test]
    fn trust_root_file_round_trips_and_defaults_to_version_1() {
        let (a, b, c) = (key(), key(), key());
        let r = TrustRoot::from_file(&file(1, &[&a, &b, &c], 2)).unwrap();
        assert_eq!(TrustRoot::from_file(&r.to_file()).unwrap(), r);
        let old: TrustRootFile = serde_json::from_str(r#"{"threshold":1,"keys":[]}"#).unwrap();
        assert_eq!(old.version, 1);
    }

    #[test]
    fn one_key_to_two_of_three_rotation() {
        let alpha = key();
        let (a, b, c) = (key(), key(), key());
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        let v2 = v1.rotate(&rotation(&file(2, &[&a, &b, &c], 2), &[&alpha, &a, &c])).unwrap();
        assert_eq!((v2.version(), v2.threshold()), (2, 2));

        // New manifests need 2 of the new keys; the alpha key alone is now worthless.
        assert!(signed_by(&body("1.0.0"), &[&b, &c]).verify(&v2, NOW).is_ok());
        assert!(signed_by(&body("1.0.0"), &[&alpha]).verify(&v2, NOW).is_err());
        assert!(signed_by(&body("1.0.0"), &[&alpha, &a]).verify(&v2, NOW).is_err());
    }

    #[test]
    fn rotation_needs_the_old_keys() {
        let alpha = key();
        let (a, b, c) = (key(), key(), key());
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        // An attacker holding new keys of their own cannot install them.
        assert_eq!(
            v1.rotate(&rotation(&file(2, &[&a, &b, &c], 2), &[&a, &b, &c])),
            Err(Error::InsufficientSignatures { valid: 0, required: 1 })
        );
    }

    #[test]
    fn rotation_needs_the_new_keys() {
        let alpha = key();
        let (a, b, c) = (key(), key(), key());
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        // Signed by the old key and only one new key: below the new threshold.
        assert_eq!(
            v1.rotate(&rotation(&file(2, &[&a, &b, &c], 2), &[&alpha, &a])),
            Err(Error::InsufficientSignatures { valid: 1, required: 2 })
        );
    }

    #[test]
    fn rotation_must_be_the_next_version() {
        let alpha = key();
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        for v in [1, 3] {
            assert_eq!(
                v1.rotate(&rotation(&file(v, &[&alpha], 1), &[&alpha])),
                Err(Error::RootVersion { expected: 2, found: v })
            );
        }
    }

    #[test]
    fn root_signature_is_not_a_manifest_signature() {
        // Same key, same bytes, different domain: must not cross over.
        let alpha = key();
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        let mut m = signed_by(&body("1.0.0"), &[]);
        add_signature(ROOT_SIGNING_CONTEXT, &m.signed.clone(), &mut m.signatures, &alpha);
        assert!(matches!(m.verify(&v1, NOW), Err(Error::InsufficientSignatures { .. })));
    }

    #[test]
    fn chain_applies_in_order_and_skips_known_links() {
        let k1 = key();
        let k2 = key();
        let k3 = key();
        let v1 = TrustRoot::single(k1.verifying_key()).unwrap();
        let r2 = rotation(&file(2, &[&k2], 1), &[&k1, &k2]);
        let r3 = rotation(&file(3, &[&k3], 1), &[&k2, &k3]);
        let latest = v1.update_chain(&[r3.clone(), r2.clone()]);
        assert_eq!((latest.root.version(), latest.skipped, latest.fork_at), (3, 0, None));
        // A client already at v3 that receives the whole chain stays at v3.
        assert_eq!(latest.root.update_chain(&[r2, r3.clone()]).root, latest.root);
        // A gap stops the walk where it is.
        let gap = v1.update_chain(&[r3]);
        assert_eq!((gap.root.version(), gap.skipped), (1, 1));
    }

    // Review of #8: a peer must not be able to block rotation with junk.

    fn alpha_to_v2() -> (SigningKey, TrustRoot, SignedRoot, SigningKey) {
        let alpha = key();
        let next = key();
        let v1 = TrustRoot::single(alpha.verifying_key()).unwrap();
        let r2 = rotation(&file(2, &[&next], 1), &[&alpha, &next]);
        (alpha, v1, r2, next)
    }

    #[test]
    fn junk_entry_does_not_block_rotation() {
        let (_, v1, r2, _) = alpha_to_v2();
        let junk = SignedRoot { signed: "{not json".into(), signatures: vec![] };
        let out = v1.update_chain(&[junk, r2]);
        assert_eq!((out.root.version(), out.skipped, out.fork_at), (2, 1, None));
    }

    #[test]
    fn forged_same_version_entry_listed_first_does_not_block_rotation() {
        let (_, v1, r2, _) = alpha_to_v2();
        let attacker = key();
        let forged = rotation(&file(2, &[&attacker], 1), &[&attacker]);
        let out = v1.update_chain(&[forged, r2]);
        assert_eq!((out.root.version(), out.skipped, out.fork_at), (2, 0, None));
    }

    #[test]
    fn padded_copy_does_not_block_the_clean_one() {
        let (_, v1, r2, _) = alpha_to_v2();
        let mut padded = r2.clone();
        let filler = padded.signatures[0].clone();
        padded.signatures.extend(std::iter::repeat(filler).take(crate::MAX_SIGNATURES));
        let out = v1.update_chain(&[padded, r2]);
        assert_eq!((out.root.version(), out.fork_at), (2, None));
    }

    #[test]
    fn same_rotation_twice_is_not_a_fork() {
        let (_, v1, r2, _) = alpha_to_v2();
        let out = v1.update_chain(&[r2.clone(), r2]);
        assert_eq!((out.root.version(), out.fork_at), (2, None));
    }

    #[test]
    fn two_valid_rotations_to_one_version_raise_the_alarm() {
        // A leaked alpha key: the thief signs their own v2 with it.
        let (alpha, v1, r2, _) = alpha_to_v2();
        let thief = key();
        let evil = rotation(&file(2, &[&thief], 1), &[&alpha, &thief]);
        let out = v1.update_chain(&[r2, evil]);
        assert_eq!(out.fork_at, Some(2));
        assert_eq!(out.root.version(), 1, "neither branch is picked");
    }

    #[test]
    fn number_of_updates_looked_at_is_capped() {
        let (_, v1, r2, _) = alpha_to_v2();
        let junk = SignedRoot { signed: "x".into(), signatures: vec![] };
        let mut updates = vec![junk; MAX_ROOT_UPDATES];
        updates.push(r2);
        assert_eq!(v1.update_chain(&updates).root.version(), 1, "entries past the cap are not read");
    }

    #[test]
    fn version_overflow_is_an_error_not_a_panic() {
        let k = key();
        let top = TrustRoot::with_version(u64::MAX, vec![k.verifying_key()], 1).unwrap();
        let r = rotation(&file(u64::MAX, &[&k], 1), &[&k]);
        assert!(matches!(top.rotate(&r), Err(Error::RootVersion { .. })));
        assert_eq!(top.update_chain(&[r]).root.version(), u64::MAX);
    }
}
