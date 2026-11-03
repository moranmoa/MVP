//! Release tooling for the signed update chain. Run without arguments for usage.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::OsRng;
use updater::{
    hash_reader, verify_artifact, ClientPolicy, Decision, ManifestBody, SignedManifest, SignedRoot, SigningKey,
    TrustRoot, TrustRootFile, Version,
};

const USAGE: &str = "\
updater - signed update manifests (spike 7)

  updater keygen --out <DIR> [--name <NAME>]
      New Ed25519 root key: <DIR>/<NAME>.key (secret) and <NAME>.pub.
      DIR must be outside any git checkout; use an encrypted volume or
      removable drive kept offline (see core/updater/SECURITY.md).
      NAME defaults to root.

  updater make-root --threshold <N> --pub <FILE.pub>... --out <root.json> [--version <V>]
      Trust root: N of the listed keys must sign. V defaults to 1.

  updater sign-root --key <FILE.key> --out <rotation.json>
                    (--root <next-root.json> | --rotation <rotation.json> --expect-root-version <V>)
      Root rotation. Needs signatures from a threshold of the current keys
      AND of the new keys; each key holder signs separately.

  updater hash --file <ARTIFACT>
      Prints the sha256 and size to paste into a manifest body.

  updater sign-manifest --key <FILE.key> --out <manifest.json>
                        (--body <body.json> | --manifest <manifest.json> --expect-version <V>)
      --body starts a new signed manifest; --manifest adds this key's
      signature to an existing one. Co-signers must name the version they
      expect, and the body is printed before signing.

  updater verify --manifest <manifest.json> --root <root.json> [--root-update <rotation.json>]...
                 [--now <UNIX>] [--channel <C> --installed <VERSION>]
                 [--artifact <FILE> --platform <P>]
      Applies root rotations, checks signatures and expiry, then the client
      rules and the artifact hash when given. Exit code 0 only if all pass.
";

/// Refuse to sign a manifest issued further in the future than this: a
/// wrong signing-machine clock would push every client's time floor ahead.
const MAX_SIGNING_SKEW_SECS: u64 = 3600;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = Args::parse(rest).and_then(|a| match cmd.as_str() {
        "keygen" => keygen(&a),
        "make-root" => make_root(&a),
        "sign-root" => sign_root(&a),
        "hash" => hash(&a),
        "sign-manifest" => sign_manifest(&a),
        "verify" => verify(&a),
        _ => Err(format!("unknown command {cmd:?}\n\n{USAGE}")),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag value` pairs; a flag may repeat.
struct Args(Vec<(String, String)>);

impl Args {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut out = Vec::new();
        let mut it = raw.iter();
        while let Some(flag) = it.next() {
            let name = flag.strip_prefix("--").ok_or_else(|| format!("expected a --flag, got {flag:?}"))?;
            let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            out.push((name.to_string(), value.clone()));
        }
        Ok(Args(out))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.0.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect()
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("missing --{name}"))
    }
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn unix_now() -> Result<u64, String> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs())
}

/// Creates `path` (never overwriting). A secret file is restricted to the
/// current user *before* anything is written into it.
fn write_new(path: &Path, contents: &str, secret: bool) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // Windows checks access when a handle is opened, so a reader that opened
    // the file before the ACL change would keep its handle. No sharing at
    // all while ours is open closes that window. WRITE_DAC lets us replace
    // the ACL through this same handle.
    #[cfg(windows)]
    if secret {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
        opts.share_mode(0).access_mode(GENERIC_WRITE | WRITE_DAC);
    }
    let mut f = opts.open(path).map_err(err)?;
    if secret {
        if let Err(e) = restrict_to_owner(&f, path) {
            drop(f);
            let _ = fs::remove_file(path);
            return Err(format!("could not restrict access to {}: {e}; no key was written", shown(path)));
        }
    }
    f.write_all(contents.as_bytes()).map_err(err)?;
    f.sync_all().map_err(err)
}

#[cfg(unix)]
fn restrict_to_owner(_file: &fs::File, _path: &Path) -> Result<(), String> {
    Ok(()) // created with mode 0600 above
}

/// Replaces the file's whole DACL, through our open handle, with a protected
/// one holding a single entry: full control for the current user's SID. No
/// inherited entries and no defaults from the token (SYSTEM, Administrators,
/// logon session) survive. The result is then checked with the system's
/// own `icacls`.
#[cfg(windows)]
fn restrict_to_owner(file: &fs::File, path: &Path) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR,
    };

    let system32 = PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?).join("System32");
    let run = |exe: &str, args: &[&std::ffi::OsStr]| -> Result<String, String> {
        let out = Command::new(system32.join(exe)).args(args).output().map_err(|e| format!("{exe}: {e}"))?;
        if !out.status.success() {
            return Err(format!("{exe} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let who = run("whoami.exe", &["/user".as_ref(), "/fo".as_ref(), "csv".as_ref(), "/nh".as_ref()])?;
    let sid = who
        .trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"'))
        .filter(|s| s.starts_with("S-1-") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .ok_or_else(|| format!("cannot read the current user's SID from {who:?}"))?
        .to_string();

    // P = protected (no inheritance); FA = full access; one ACE for our SID.
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;{sid})").encode_utf16().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` is freed with LocalFree below.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == 0 {
        return Err(format!("building the ACL failed: {}", std::io::Error::last_os_error()));
    }
    let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
    // SAFETY: `sd` is a valid descriptor from the call above; `dacl` points into it.
    let rc = unsafe {
        if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 || present == 0 {
            Err(format!("reading the ACL failed: {}", std::io::Error::last_os_error()))
        } else {
            match SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            ) {
                ERROR_SUCCESS => Ok(()),
                code => Err(format!("setting the ACL failed: {}", std::io::Error::from_raw_os_error(code as i32))),
            }
        }
    };
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd) };
    rc?;

    // Independent check: icacls lists one ACE per line after the path.
    let listing = run("icacls.exe", &[path.as_os_str()])?;
    let aces = listing.lines().filter(|l| l.contains(":(")).count();
    if aces != 1 {
        return Err(format!("expected exactly one access entry, icacls shows {aces}:\n{listing}"));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict_to_owner(_file: &fs::File, _path: &Path) -> Result<(), String> {
    Err("don't know how to restrict file access on this platform".into())
}

/// A path for humans: without the `\\?\` prefix `canonicalize` adds on Windows.
fn shown(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// The nearest ancestor of `dir` (inclusive) that holds a `.git` entry.
fn enclosing_git_checkout(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

fn keygen(a: &Args) -> Result<(), String> {
    let dir = fs::canonicalize(a.need("out")?).map_err(|e| format!("--out: {e}"))?;
    if let Some(repo) = enclosing_git_checkout(&dir) {
        return Err(format!(
            "refusing to write a secret key inside the git checkout {}; pick a folder outside any repository",
            shown(&repo)
        ));
    }
    let name = a.get("name").unwrap_or("root");
    if !updater::is_valid_name(name) {
        return Err(format!("bad --name {name:?}: use a-z, 0-9, - and _"));
    }
    let key = SigningKey::generate(&mut OsRng);
    let public = hex::encode(key.verifying_key().to_bytes());
    let secret_path = dir.join(format!("{name}.key"));
    let public_path = dir.join(format!("{name}.pub"));
    write_new(&secret_path, &format!("{}\n", hex::encode(key.to_bytes())), true)?;
    write_new(&public_path, &format!("{public}\n"), false)?;
    println!("secret key: {} (readable by you only)", shown(&secret_path));
    println!("public key: {} ({public})", shown(&public_path));
    println!("keep the .key file offline: encrypted volume or removable drive, never in cloud-synced folders");
    Ok(())
}

fn load_secret(path: &str) -> Result<SigningKey, String> {
    let text = String::from_utf8(read(path)?).map_err(|_| format!("{path}: not text"))?;
    let bytes: [u8; 32] = hex::decode(text.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("{path}: not a hex Ed25519 secret key"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn load_root(path: &str) -> Result<TrustRoot, String> {
    let file: TrustRootFile = serde_json::from_slice(&read(path)?).map_err(|e| format!("{path}: {e}"))?;
    TrustRoot::from_file(&file).map_err(|e| format!("{path}: {e}"))
}

fn make_root(a: &Args) -> Result<(), String> {
    let threshold: usize = a.need("threshold")?.parse().map_err(|_| "--threshold must be a number".to_string())?;
    let version: u64 = a.get("version").unwrap_or("1").parse().map_err(|_| "--version must be a number".to_string())?;
    let keys = a
        .all("pub")
        .into_iter()
        .map(|p| Ok(String::from_utf8(read(p)?).map_err(|_| format!("{p}: not text"))?.trim().to_string()))
        .collect::<Result<Vec<_>, String>>()?;
    let file = TrustRootFile { version, threshold, keys };
    TrustRoot::from_file(&file).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&file).expect("serialises");
    write_new(Path::new(a.need("out")?), &format!("{json}\n"), false)?;
    println!("trust root v{version}: {threshold} of {} keys", file.keys.len());
    Ok(())
}

fn sign_root(a: &Args) -> Result<(), String> {
    let key = load_secret(a.need("key")?)?;
    let mut rotation = match (a.get("root"), a.get("rotation")) {
        (Some(root), None) => {
            let next: TrustRootFile = serde_json::from_slice(&read(root)?).map_err(|e| format!("{root}: {e}"))?;
            SignedRoot::new(&next).map_err(|e| e.to_string())?
        }
        (None, Some(r)) => {
            let rotation = SignedRoot::from_json(&read(r)?).map_err(|e| e.to_string())?;
            let next = rotation.body().map_err(|e| e.to_string())?;
            TrustRoot::from_file(&next).map_err(|e| e.to_string())?;
            let expected = a.need("expect-root-version")?;
            if next.version.to_string() != expected {
                return Err(format!("rotation is to root v{}, you expected v{expected}; not signing", next.version));
            }
            rotation
        }
        _ => return Err("give exactly one of --root or --rotation".into()),
    };
    let next = rotation.body().map_err(|e| e.to_string())?;
    println!("signing rotation to root v{}: {} of these keys:", next.version, next.threshold);
    for k in &next.keys {
        println!("  {k}");
    }
    rotation.sign(&key);
    let out = a.need("out")?;
    fs::write(out, format!("{}\n", rotation.to_json())).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}: {} signature(s)", rotation.signatures.len());
    Ok(())
}

fn hash(a: &Args) -> Result<(), String> {
    let path = a.need("file")?;
    let f = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let (sha256, size) = hash_reader(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
    println!("{}", serde_json::json!({ "sha256": sha256, "size": size }));
    Ok(())
}

fn print_body(body: &ManifestBody) {
    println!(
        "manifest: channel {}, version {}, issued {}, expires {}",
        body.channel, body.version, body.issued_at, body.expires_at
    );
    for (platform, a) in &body.artifacts {
        println!("  {platform}: {} ({} bytes)", a.sha256, a.size);
    }
    if !body.revoked.is_empty() {
        let revoked: Vec<String> = body.revoked.iter().map(ToString::to_string).collect();
        println!("  revokes: {}", revoked.join(", "));
    }
}

fn sign_manifest(a: &Args) -> Result<(), String> {
    let key = load_secret(a.need("key")?)?;
    let mut manifest = match (a.get("body"), a.get("manifest")) {
        (Some(body), None) => {
            let body: ManifestBody = serde_json::from_slice(&read(body)?).map_err(|e| format!("body: {e}"))?;
            SignedManifest::new(&body).map_err(|e| e.to_string())?
        }
        (None, Some(m)) => {
            let manifest = SignedManifest::from_json(&read(m)?).map_err(|e| e.to_string())?;
            let body = manifest.body_unverified().map_err(|e| e.to_string())?;
            let expected = a.need("expect-version")?;
            if body.version.to_string() != expected {
                return Err(format!("manifest is for {}, you expected {expected}; not signing", body.version));
            }
            manifest
        }
        _ => return Err("give exactly one of --body or --manifest".into()),
    };
    let body = manifest.body_unverified().map_err(|e| e.to_string())?;
    let now = unix_now()?;
    if body.issued_at > now + MAX_SIGNING_SKEW_SECS {
        return Err(format!("issued_at {} is in the future (now {now}); check this machine's clock", body.issued_at));
    }
    if body.expires_at <= now {
        return Err(format!("expires_at {} is already past (now {now})", body.expires_at));
    }
    print_body(&body);
    manifest.sign(&key);
    let out = a.need("out")?;
    fs::write(out, format!("{}\n", manifest.to_json())).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}: {} signature(s)", manifest.signatures.len());
    Ok(())
}

fn verify(a: &Args) -> Result<(), String> {
    let manifest = SignedManifest::from_json(&read(a.need("manifest")?)?).map_err(|e| e.to_string())?;
    let mut root = load_root(a.need("root")?)?;
    let updates = a
        .all("root-update")
        .into_iter()
        .map(|p| SignedRoot::from_json(&read(p)?).map_err(|e| format!("{p}: {e}")))
        .collect::<Result<Vec<_>, String>>()?;
    if !updates.is_empty() {
        let chain = root.update_chain(&updates);
        if let Some(v) = chain.fork_at {
            return Err(format!(
                "two different validly signed rotations to root v{v}: a current root key has leaked; not choosing one"
            ));
        }
        if chain.skipped > 0 {
            println!("warning: {} root update(s) could not be applied (invalid, forged or after a gap)", chain.skipped);
        }
        root = chain.root;
        println!("trust root now v{} ({} of its keys must sign)", root.version(), root.threshold());
    }
    let now = match a.get("now") {
        Some(n) => n.parse().map_err(|_| "--now must be Unix seconds".to_string())?,
        None => unix_now()?,
    };

    let verified = manifest.verify(&root, now).map_err(|e| e.to_string())?;
    let body = verified.body();
    println!("signatures ok, channel {}, version {}, expires {}", body.channel, body.version, body.expires_at);

    if let Some(installed) = a.get("installed") {
        let installed: Version = installed.parse().map_err(|e: updater::ParseVersionError| e.to_string())?;
        let mut policy = ClientPolicy::new(a.get("channel").unwrap_or(&body.channel));
        match policy.accept(&verified, installed).map_err(|e| e.to_string())? {
            Decision::UpToDate => println!("installed {installed} is up to date"),
            Decision::Install(v) => println!("update {installed} -> {v} allowed"),
        }
        if policy.is_revoked(installed) {
            println!("warning: installed version {installed} is revoked");
        }
    }

    if let Some(path) = a.get("artifact") {
        let platform = a.need("platform")?;
        let f = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        verify_artifact(&verified, platform, std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
        println!("artifact {path} matches {platform}");
    }

    if !body.revoked.is_empty() {
        let revoked: Vec<String> = body.revoked.iter().map(ToString::to_string).collect();
        println!("revoked: {}", revoked.join(", "));
    }
    Ok(())
}
