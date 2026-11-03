//! Artifact hash checks. Run before installing an artifact *and* before
//! passing it on to another peer, however the bytes arrived.
//!
//! Time-of-check / time-of-use: checking a file by path and then installing
//! it by path again leaves a window in which the file can be swapped. So the
//! intended flow is:
//!
//! 1. [`store_artifact`] streams the incoming bytes into a fresh file in the
//!    app's private update directory, hashing while writing, and only gives
//!    it its final name once size and hash match.
//! 2. [`StoredArtifact::open_verified`] opens that file, re-hashes it through
//!    the same handle and rewinds it. The installer (and the code that
//!    serves the artifact to peers) reads only from that handle, never
//!    from the path.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::{Error, Version, VerifiedManifest};

/// Proof that an artifact matched a verified manifest. Only this module
/// makes one (fields are private), and [`crate::InstallState::stage`]
/// requires it, so nothing unverified can be staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedArtifact {
    version: Version,
    platform: String,
    sha256: String,
    size: u64,
}

impl VerifiedArtifact {
    pub fn version(&self) -> Version {
        self.version
    }

    pub fn platform(&self) -> &str {
        &self.platform
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    #[cfg(test)]
    pub(crate) fn for_tests(version: Version, platform: &str) -> Self {
        VerifiedArtifact { version, platform: platform.into(), sha256: "00".repeat(32), size: 1 }
    }
}

/// SHA-256 and size of a stream, read once.
pub fn hash_reader<R: Read>(reader: R) -> Result<(String, u64), Error> {
    hash_copy(reader, &mut std::io::sink())
}

/// Hashes `reader` while copying it into `out`.
fn hash_copy<R: Read, W: Write>(mut reader: R, out: &mut W) -> Result<(String, u64), Error> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

fn check(m: &VerifiedManifest, platform: &str, sha256: String, size: u64) -> Result<VerifiedArtifact, Error> {
    let entry = &m.body().artifacts[platform];
    if size != entry.size {
        return Err(Error::SizeMismatch { expected: entry.size, actual: size });
    }
    if sha256 != entry.sha256 {
        return Err(Error::HashMismatch);
    }
    Ok(VerifiedArtifact { version: m.version(), platform: platform.to_string(), sha256, size })
}

/// The expected size for `platform`, plus one: reading that many bytes is
/// enough to detect an oversized artifact from a hostile peer.
fn read_cap(m: &VerifiedManifest, platform: &str) -> Result<u64, Error> {
    let entry = m.body().artifacts.get(platform).ok_or_else(|| Error::NoArtifact(platform.to_string()))?;
    Ok(entry.size.saturating_add(1))
}

/// Streams `reader` and checks it against the manifest entry for `platform`.
/// Prefer [`store_artifact`] when the bytes are going to be installed.
pub fn verify_artifact<R: Read>(m: &VerifiedManifest, platform: &str, reader: R) -> Result<VerifiedArtifact, Error> {
    let cap = read_cap(m, platform)?;
    let (sha256, size) = hash_reader(reader.take(cap))?;
    check(m, platform, sha256, size)
}

/// A verified artifact at rest in the update directory.
#[derive(Debug)]
pub struct StoredArtifact {
    artifact: VerifiedArtifact,
    path: PathBuf,
}

/// Streams `reader` into `dir/<version>-<platform>.bin`, verifying on the
/// way. On any failure nothing is left under the final name. `dir` must be
/// private to the app's user.
pub fn store_artifact<R: Read>(
    m: &VerifiedManifest,
    platform: &str,
    reader: R,
    dir: &Path,
) -> Result<StoredArtifact, Error> {
    let cap = read_cap(m, platform)?;
    // `platform` is a validated name (see `is_valid_name`), so it is safe in a file name.
    let final_path = dir.join(format!("{}-{platform}.bin", m.version()));
    let mut nonce = [0u8; 8];
    OsRng.fill_bytes(&mut nonce);
    let tmp_path = dir.join(format!(".incoming-{}", hex::encode(nonce)));

    let result = (|| {
        let mut tmp = OpenOptions::new().write(true).create_new(true).open(&tmp_path)?;
        let (sha256, size) = hash_copy(reader.take(cap), &mut tmp)?;
        let artifact = check(m, platform, sha256, size)?;
        tmp.sync_all()?;
        drop(tmp);
        if final_path.exists() {
            fs::remove_file(&final_path)?;
        }
        fs::rename(&tmp_path, &final_path)?;
        Ok(artifact)
    })();
    match result {
        Ok(artifact) => Ok(StoredArtifact { artifact, path: final_path }),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

impl StoredArtifact {
    pub fn artifact(&self) -> &VerifiedArtifact {
        &self.artifact
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Opens the stored file, re-hashes it through the returned handle and
    /// rewinds it. Install from (or serve to peers from) this handle only.
    pub fn open_verified(&self) -> Result<File, Error> {
        let mut f = File::open(&self.path)?;
        let (sha256, size) = hash_reader((&mut f).take(self.artifact.size.saturating_add(1)))?;
        if size != self.artifact.size {
            return Err(Error::SizeMismatch { expected: self.artifact.size, actual: size });
        }
        if sha256 != self.artifact.sha256 {
            return Err(Error::HashMismatch);
        }
        f.seek(SeekFrom::Start(0))?;
        Ok(f)
    }
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

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut n = [0u8; 4];
            OsRng.fill_bytes(&mut n);
            let p = std::env::temp_dir().join(format!("updater-art-{tag}-{}", hex::encode(n)));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }

        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> =
                fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            v.sort();
            v
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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
        assert_eq!(a.version(), Version::new(1, 1, 0));
        assert_eq!(a.size(), PAYLOAD.len() as u64);
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

    #[test]
    fn stored_artifact_lands_under_its_final_name() {
        let dir = TempDir::new("ok");
        let m = manifest_for(PAYLOAD);
        let s = store_artifact(&m, "windows-x86_64", PAYLOAD, &dir.0).unwrap();
        assert_eq!(dir.entries(), vec!["1.1.0-windows-x86_64.bin".to_string()]);
        let mut back = Vec::new();
        s.open_verified().unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, PAYLOAD);
    }

    #[test]
    fn bad_download_leaves_nothing_behind() {
        let dir = TempDir::new("bad");
        let m = manifest_for(PAYLOAD);
        let mut bad = PAYLOAD.to_vec();
        bad[0] ^= 1;
        assert_eq!(store_artifact(&m, "windows-x86_64", &bad[..], &dir.0).unwrap_err(), Error::HashMismatch);
        assert!(store_artifact(&m, "windows-x86_64", std::io::repeat(7), &dir.0).is_err());
        assert!(dir.entries().is_empty(), "temp files must be cleaned up: {:?}", dir.entries());
    }

    #[test]
    fn swapped_file_after_verification_is_caught() {
        // TOCTOU: the file is replaced between store and install.
        let dir = TempDir::new("swap");
        let m = manifest_for(PAYLOAD);
        let s = store_artifact(&m, "windows-x86_64", PAYLOAD, &dir.0).unwrap();
        let mut evil = PAYLOAD.to_vec();
        evil[5] ^= 0x20;
        fs::write(s.path(), &evil).unwrap();
        assert_eq!(s.open_verified().unwrap_err(), Error::HashMismatch);
        fs::write(s.path(), [PAYLOAD, b"tail"].concat()).unwrap();
        assert!(matches!(s.open_verified().unwrap_err(), Error::SizeMismatch { .. }));
    }

    #[test]
    fn replaces_a_stale_file_with_the_same_name() {
        let dir = TempDir::new("replace");
        let m = manifest_for(PAYLOAD);
        fs::write(dir.0.join("1.1.0-windows-x86_64.bin"), b"leftover junk").unwrap();
        let s = store_artifact(&m, "windows-x86_64", PAYLOAD, &dir.0).unwrap();
        assert!(s.open_verified().is_ok());
    }
}
