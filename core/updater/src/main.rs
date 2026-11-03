//! Release tooling for the signed update chain. Run without arguments for usage.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::OsRng;
use updater::{
    hash_reader, verify_artifact, ClientPolicy, Decision, ManifestBody, SignedManifest, SigningKey, TrustRoot,
    TrustRootFile, Version,
};

const USAGE: &str = "\
updater - signed update manifests (spike 7)

  updater keygen --out <DIR> [--name <NAME>]
      New Ed25519 root key: <DIR>/<NAME>.key (secret) and <NAME>.pub.
      DIR must be outside any git checkout. NAME defaults to root.

  updater make-root --threshold <N> --pub <FILE.pub>... --out <root.json>
      Trust root the app embeds: N of the listed keys must sign.

  updater hash --file <ARTIFACT>
      Prints the sha256 and size to paste into a manifest body.

  updater sign-manifest --key <FILE.key> --out <manifest.json> (--body <body.json> | --manifest <manifest.json>)
      --body starts a new signed manifest; --manifest adds this key's
      signature to an existing one (one signer per key holder for 2-of-3).

  updater verify --manifest <manifest.json> --root <root.json> [--now <UNIX>]
                 [--channel <C> --installed <VERSION>] [--artifact <FILE> --platform <P>]
      Checks signatures and expiry, then the client rules and the artifact
      hash when given. Exit code 0 only if everything passes.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = Args::parse(rest).and_then(|a| match cmd.as_str() {
        "keygen" => keygen(&a),
        "make-root" => make_root(&a),
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

fn write_new(path: &Path, contents: &str, secret: bool) -> Result<(), String> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut f = opts.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(contents.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
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
            repo.display()
        ));
    }
    let name = a.get("name").unwrap_or("root");
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("bad --name {name:?}"));
    }
    let key = SigningKey::generate(&mut OsRng);
    let public = hex::encode(key.verifying_key().to_bytes());
    let secret_path = dir.join(format!("{name}.key"));
    let public_path = dir.join(format!("{name}.pub"));
    write_new(&secret_path, &format!("{}\n", hex::encode(key.to_bytes())), true)?;
    write_new(&public_path, &format!("{public}\n"), false)?;
    println!("secret key: {}", secret_path.display());
    println!("public key: {} ({public})", public_path.display());
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

fn make_root(a: &Args) -> Result<(), String> {
    let threshold: usize = a.need("threshold")?.parse().map_err(|_| "--threshold must be a number".to_string())?;
    let keys = a
        .all("pub")
        .into_iter()
        .map(|p| Ok(String::from_utf8(read(p)?).map_err(|_| format!("{p}: not text"))?.trim().to_string()))
        .collect::<Result<Vec<_>, String>>()?;
    let file = TrustRootFile { threshold, keys };
    TrustRoot::from_file(&file).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&file).expect("serialises");
    write_new(Path::new(a.need("out")?), &format!("{json}\n"), false)?;
    println!("trust root: {threshold} of {} keys", file.keys.len());
    Ok(())
}

fn hash(a: &Args) -> Result<(), String> {
    let path = a.need("file")?;
    let f = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let (sha256, size) = hash_reader(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
    println!("{}", serde_json::json!({ "sha256": sha256, "size": size }));
    Ok(())
}

fn sign_manifest(a: &Args) -> Result<(), String> {
    let key = load_secret(a.need("key")?)?;
    let mut manifest = match (a.get("body"), a.get("manifest")) {
        (Some(body), None) => {
            let body: ManifestBody = serde_json::from_slice(&read(body)?).map_err(|e| format!("body: {e}"))?;
            SignedManifest::new(&body).map_err(|e| e.to_string())?
        }
        (None, Some(m)) => SignedManifest::from_json(&read(m)?).map_err(|e| e.to_string())?,
        _ => return Err("give exactly one of --body or --manifest".into()),
    };
    manifest.sign(&key);
    let out = a.need("out")?;
    fs::write(out, format!("{}\n", manifest.to_json())).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}: {} signature(s)", manifest.signatures.len());
    Ok(())
}

fn verify(a: &Args) -> Result<(), String> {
    let manifest = SignedManifest::from_json(&read(a.need("manifest")?)?).map_err(|e| e.to_string())?;
    let root_file: TrustRootFile =
        serde_json::from_slice(&read(a.need("root")?)?).map_err(|e| format!("root: {e}"))?;
    let root = TrustRoot::from_file(&root_file).map_err(|e| e.to_string())?;
    let now = match a.get("now") {
        Some(n) => n.parse().map_err(|_| "--now must be Unix seconds".to_string())?,
        None => SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs(),
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
