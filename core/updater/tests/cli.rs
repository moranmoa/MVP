//! End-to-end release flow through the CLI: keygen, 2-of-3 signing, verify.
//! All keys live in a fresh temp folder and are deleted afterwards.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn updater(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_updater")).args(args).output().unwrap()
}

fn ok(args: &[&str]) -> String {
    let out = updater(args);
    assert!(out.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn fails(args: &[&str]) -> String {
    let out = updater(args);
    assert!(!out.status.success(), "{args:?} should have failed");
    String::from_utf8(out.stderr).unwrap()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("updater-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_body(dir: &TempDir, artifact: &Path, version: &str, revoked: &[&str]) -> String {
    let hash = ok(&["hash", "--file", artifact.to_str().unwrap()]);
    let entry: serde_json::Value = serde_json::from_str(&hash).unwrap();
    let body = serde_json::json!({
        "format": 1,
        "channel": "stable",
        "version": version,
        "issued_at": now() - 100,
        "expires_at": now() + 7 * 86_400,
        "artifacts": { "linux-x86_64": entry },
        "revoked": revoked,
    });
    let path = dir.path(&format!("body-{version}.json"));
    fs::write(&path, body.to_string()).unwrap();
    path
}

#[test]
fn two_of_three_release_flow() {
    let dir = TempDir::new("flow");
    for n in ["a", "b", "c"] {
        ok(&["keygen", "--out", dir.0.to_str().unwrap(), "--name", n]);
    }
    let root = dir.path("root.json");
    ok(&["make-root", "--threshold", "2", "--pub", &dir.path("a.pub"), "--pub", &dir.path("b.pub"),
         "--pub", &dir.path("c.pub"), "--out", &root]);

    let artifact = dir.0.join("app.bin");
    fs::write(&artifact, b"release 1.1.0 bytes").unwrap();
    let body = write_body(&dir, &artifact, "1.1.0", &["1.0.1"]);
    let manifest = dir.path("manifest.json");

    ok(&["sign-manifest", "--key", &dir.path("a.key"), "--body", &body, "--out", &manifest]);
    let err = fails(&["verify", "--manifest", &manifest, "--root", &root]);
    assert!(err.contains("1 of 2"), "{err}");

    ok(&["sign-manifest", "--key", &dir.path("c.key"), "--manifest", &manifest, "--expect-version", "1.1.0", "--out", &manifest]);
    let out = ok(&["verify", "--manifest", &manifest, "--root", &root,
                   "--installed", "1.0.0", "--artifact", artifact.to_str().unwrap(), "--platform", "linux-x86_64"]);
    assert!(out.contains("update 1.0.0 -> 1.1.0 allowed"), "{out}");
    assert!(out.contains("matches linux-x86_64"), "{out}");

    // Downgrade, revoked install and expiry are refused.
    fails(&["verify", "--manifest", &manifest, "--root", &root, "--installed", "1.2.0"]);
    let out = ok(&["verify", "--manifest", &manifest, "--root", &root, "--installed", "1.0.1"]);
    assert!(out.contains("installed version 1.0.1 is revoked"), "{out}");
    let err = fails(&["verify", "--manifest", &manifest, "--root", &root, "--now", &(now() + 8 * 86_400).to_string()]);
    assert!(err.contains("expired"), "{err}");

    // A tampered artifact is refused.
    let bad = dir.0.join("bad.bin");
    fs::write(&bad, b"release 1.1.0 byteZ").unwrap();
    let err = fails(&["verify", "--manifest", &manifest, "--root", &root,
                      "--artifact", bad.to_str().unwrap(), "--platform", "linux-x86_64"]);
    assert!(err.contains("hash"), "{err}");
}

#[test]
fn single_key_closed_alpha_flow() {
    let dir = TempDir::new("single");
    ok(&["keygen", "--out", dir.0.to_str().unwrap()]);
    let root = dir.path("root.json");
    ok(&["make-root", "--threshold", "1", "--pub", &dir.path("root.pub"), "--out", &root]);
    let artifact = dir.0.join("app.bin");
    fs::write(&artifact, b"x").unwrap();
    let manifest = dir.path("m.json");
    ok(&["sign-manifest", "--key", &dir.path("root.key"), "--body", &write_body(&dir, &artifact, "0.1.0", &[]),
         "--out", &manifest]);
    ok(&["verify", "--manifest", &manifest, "--root", &root]);
}

#[test]
fn keygen_refuses_git_checkouts_and_overwrites() {
    let dir = TempDir::new("keygen");
    let repo = dir.0.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("sub")).unwrap();
    let err = fails(&["keygen", "--out", repo.join("sub").to_str().unwrap()]);
    assert!(err.contains("git checkout"), "{err}");
    assert!(!repo.join("sub").join("root.key").exists());

    ok(&["keygen", "--out", dir.0.to_str().unwrap()]);
    let first = fs::read(dir.0.join("root.key")).unwrap();
    fails(&["keygen", "--out", dir.0.to_str().unwrap()]);
    assert_eq!(fs::read(dir.0.join("root.key")).unwrap(), first, "existing key must not be overwritten");
}

#[cfg(unix)]
#[test]
fn secret_key_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("perm");
    ok(&["keygen", "--out", dir.0.to_str().unwrap()]);
    let mode = fs::metadata(dir.0.join("root.key")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn co_signer_must_name_the_version_they_sign() {
    let dir = TempDir::new("cosign");
    for n in ["a", "b"] {
        ok(&["keygen", "--out", dir.0.to_str().unwrap(), "--name", n]);
    }
    let artifact = dir.0.join("app.bin");
    fs::write(&artifact, b"x").unwrap();
    let manifest = dir.path("m.json");
    ok(&["sign-manifest", "--key", &dir.path("a.key"), "--body", &write_body(&dir, &artifact, "2.0.0", &[]),
         "--out", &manifest]);
    let (b_key, out) = (dir.path("b.key"), dir.path("m2.json"));
    let co = |extra: &[&str]| {
        let mut args = vec!["sign-manifest", "--key", &b_key, "--manifest", &manifest, "--out", &out];
        args.extend_from_slice(extra);
        updater(&args)
    };
    assert!(!co(&[]).status.success(), "co-signing without --expect-version must fail");
    let wrong = co(&["--expect-version", "1.9.0"]);
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("not signing"));
    let right = co(&["--expect-version", "2.0.0"]);
    assert!(right.status.success());
    assert!(String::from_utf8_lossy(&right.stdout).contains("version 2.0.0"), "body is shown before signing");
}

#[test]
fn refuses_to_sign_a_body_from_the_future() {
    let dir = TempDir::new("future");
    ok(&["keygen", "--out", dir.0.to_str().unwrap()]);
    let body = serde_json::json!({
        "format": 1, "channel": "stable", "version": "1.0.0",
        "issued_at": now() + 86_400, "expires_at": now() + 8 * 86_400,
        "artifacts": { "linux-x86_64": { "sha256": "ab".repeat(32), "size": 1 } },
    });
    let path = dir.path("body.json");
    fs::write(&path, body.to_string()).unwrap();
    let err = fails(&["sign-manifest", "--key", &dir.path("root.key"), "--body", &path, "--out", &dir.path("m.json")]);
    assert!(err.contains("clock"), "{err}");
}

#[test]
fn one_key_to_two_of_three_rotation_flow() {
    let dir = TempDir::new("rotate");
    for n in ["alpha", "a", "b", "c"] {
        ok(&["keygen", "--out", dir.0.to_str().unwrap(), "--name", n]);
    }
    let v1 = dir.path("root-v1.json");
    ok(&["make-root", "--threshold", "1", "--pub", &dir.path("alpha.pub"), "--out", &v1]);
    let v2 = dir.path("root-v2.json");
    ok(&["make-root", "--version", "2", "--threshold", "2", "--pub", &dir.path("a.pub"), "--pub", &dir.path("b.pub"),
         "--pub", &dir.path("c.pub"), "--out", &v2]);
    let rot = dir.path("rotation-2.json");
    ok(&["sign-root", "--key", &dir.path("alpha.key"), "--root", &v2, "--out", &rot]);
    for k in ["a", "b"] {
        ok(&["sign-root", "--key", &dir.path(&format!("{k}.key")), "--rotation", &rot, "--expect-root-version", "2",
             "--out", &rot]);
    }

    let artifact = dir.0.join("app.bin");
    fs::write(&artifact, b"x").unwrap();
    let body = write_body(&dir, &artifact, "1.0.0", &[]);
    let old_only = dir.path("old.json");
    ok(&["sign-manifest", "--key", &dir.path("alpha.key"), "--body", &body, "--out", &old_only]);
    // Before the rotation the alpha key works; after it, it does not.
    ok(&["verify", "--manifest", &old_only, "--root", &v1]);
    fails(&["verify", "--manifest", &old_only, "--root", &v1, "--root-update", &rot]);

    let new = dir.path("new.json");
    ok(&["sign-manifest", "--key", &dir.path("b.key"), "--body", &body, "--out", &new]);
    ok(&["sign-manifest", "--key", &dir.path("c.key"), "--manifest", &new, "--expect-version", "1.0.0", "--out", &new]);
    let out = ok(&["verify", "--manifest", &new, "--root", &v1, "--root-update", &rot]);
    assert!(out.contains("trust root now v2"), "{out}");
}

#[cfg(windows)]
#[test]
fn secret_key_acl_is_owner_only_on_windows() {
    let dir = TempDir::new("acl");
    ok(&["keygen", "--out", dir.0.to_str().unwrap()]);
    let out = Command::new("icacls").arg(dir.0.join("root.key")).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().filter(|l| l.contains(":(")).count(), 1, "exactly one ACE expected:\n{text}");
    assert!(!text.contains("(I)"), "no inherited ACEs expected:\n{text}");
    // The public key keeps normal, inherited permissions.
    let public = Command::new("icacls").arg(dir.0.join("root.pub")).output().unwrap();
    assert!(String::from_utf8_lossy(&public.stdout).contains("(I)"));
}
