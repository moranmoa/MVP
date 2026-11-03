//! Artifact hash checks. Run before installing an artifact *and* before
//! passing it on to another peer, however the bytes arrived.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::{Error, Version, VerifiedManifest};

/// Proof that an artifact matched a verified manifest. Only
/// [`verify_artifact`] makes one, and [`crate::InstallState::stage`]
/// requires it, so nothing unverified can be staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub version: Version,
    pub platform: String,
    pub sha256: String,
    pub size: u64,
}

/// SHA-256 and size of a stream, read once.
pub fn hash_reader<R: Read>(mut reader: R) -> Result<(String, u64), Error> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// Streams `reader` and checks it against the manifest entry for `platform`.
/// Reads at most one byte past the expected size, so an oversized artifact
/// from a hostile peer is cut off early.
pub fn verify_artifact<R: Read>(m: &VerifiedManifest, platform: &str, reader: R) -> Result<VerifiedArtifact, Error> {
    let entry = m.body().artifacts.get(platform).ok_or_else(|| Error::NoArtifact(platform.to_string()))?;
    let (sha256, size) = hash_reader(reader.take(entry.size.saturating_add(1)))?;
    if size != entry.size {
        return Err(Error::SizeMismatch { expected: entry.size, actual: size });
    }
    if sha256 != entry.sha256 {
        return Err(Error::HashMismatch);
    }
    Ok(VerifiedArtifact { version: m.version(), platform: platform.to_string(), sha256, size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{body, key, signed_by, NOW};
    use crate::{Artifact, TrustRoot};

    const PAYLOAD: &[u8] = b"pretend this is an installer";

    fn manifest_for(payload: &[u8]) -> VerifiedManifest {
        let k = key();
        let mut b = body("1.1.0");
        let (sha256, size) = hash_reader(payload).unwrap();
        b.artifacts.insert("windows-x86_64".into(), Artifact { sha256, size });
        signed_by(&b, &[&k]).verify(&TrustRoot::single(k.verifying_key()).unwrap(), NOW).unwrap()
    }

    #[test]
    fn known_sha256_vector() {
        assert_eq!(
            hash_reader(&b"abc"[..]).unwrap(),
            ("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(), 3)
        );
    }

    #[test]
    fn matching_artifact_is_verified() {
        let m = manifest_for(PAYLOAD);
        let a = verify_artifact(&m, "windows-x86_64", PAYLOAD).unwrap();
        assert_eq!(a.version, Version::new(1, 1, 0));
        assert_eq!(a.size, PAYLOAD.len() as u64);
    }

    #[test]
    fn one_flipped_bit_is_rejected() {
        let m = manifest_for(PAYLOAD);
        let mut bad = PAYLOAD.to_vec();
        bad[3] ^= 0x01;
        assert_eq!(verify_artifact(&m, "windows-x86_64", &bad[..]), Err(Error::HashMismatch));
    }

    #[test]
    fn truncated_and_extended_artifacts_are_rejected() {
        let m = manifest_for(PAYLOAD);
        let n = PAYLOAD.len() as u64;
        assert_eq!(
            verify_artifact(&m, "windows-x86_64", &PAYLOAD[..PAYLOAD.len() - 1]),
            Err(Error::SizeMismatch { expected: n, actual: n - 1 })
        );
        let long = [PAYLOAD, b"!!! extra"].concat();
        assert_eq!(
            verify_artifact(&m, "windows-x86_64", &long[..]),
            Err(Error::SizeMismatch { expected: n, actual: n + 1 })
        );
    }

    #[test]
    fn oversized_stream_is_not_read_to_the_end() {
        let m = manifest_for(PAYLOAD);
        let endless = std::io::repeat(0u8); // would never finish without the cap
        assert!(matches!(verify_artifact(&m, "windows-x86_64", endless), Err(Error::SizeMismatch { .. })));
    }

    #[test]
    fn unknown_platform_is_rejected() {
        let m = manifest_for(PAYLOAD);
        assert_eq!(verify_artifact(&m, "macos-arm64", PAYLOAD), Err(Error::NoArtifact("macos-arm64".into())));
    }

    #[test]
    fn artifact_for_another_platform_does_not_pass() {
        let m = manifest_for(PAYLOAD);
        // linux entry in the test body has a different hash
        assert!(verify_artifact(&m, "linux-x86_64", PAYLOAD).is_err());
    }
}
