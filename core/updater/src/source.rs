//! Where update files come from (B8-4): the `UpdateSource` trait, and
//! `HttpsSource` for the project's GitHub release assets.
//!
//! A source is only transport. Everything it returns is checked by the
//! caller (signatures, policy, hash, size), so TLS here is for privacy, not
//! integrity. A later P2P source implements the same trait.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use crate::manifest::is_valid_name;
use crate::root::{MAX_ROOT_FETCH_BYTES, MAX_ROOT_UPDATES_PER_FETCH, MAX_ROOT_UPDATE_BYTES};
use crate::{Version, MAX_MANIFEST_BYTES};

/// Why a source could not deliver a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// No connection, DNS failure, timeout, TLS failure.
    Offline(String),
    /// The body is larger than the caller allowed.
    TooLarge,
    /// The file does not exist (HTTP 404).
    NotFound,
    /// Anything else the source refused or broke on (a bad redirect, an
    /// unexpected status).
    Refused(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Offline(why) => write!(f, "offline: {why}"),
            SourceError::TooLarge => write!(f, "response too large"),
            SourceError::NotFound => write!(f, "not found"),
            SourceError::Refused(why) => write!(f, "refused: {why}"),
        }
    }
}

/// The transport for update files.
pub trait UpdateSource: Send + Sync {
    /// The signed manifest for `channel`, at most `MAX_MANIFEST_BYTES`.
    fn manifest(&self, channel: &str) -> Result<Vec<u8>, SourceError>;
    /// Signed root rotations, oldest first. The caller applies
    /// `parse_root_fetch` (GAP-12); a source should stay inside its caps.
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError>;
    /// Streams the payload into `out`, failing with `TooLarge` after
    /// `max_len` bytes.
    fn artifact(&self, version: Version, platform: &str, max_len: u64, out: &mut dyn Write)
        -> Result<(), SourceError>;
}

/// Release assets of the project repository.
pub const RELEASE_BASE: &str = "https://github.com/moranmoa/MVP/releases/download/";

/// The only hosts a download may touch (A4): GitHub itself and the exact
/// hosts it redirects release assets to. Not `*.githubusercontent.com`:
/// `raw.` and `user-images.` serve anyone's content.
pub const ALLOWED_HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// Most redirects followed for one file.
pub const MAX_REDIRECTS: usize = 4;

/// Whether `url` may be fetched: `https`, an allowed host, the default port,
/// no user info.
pub fn is_allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = authority.strip_suffix(":443").unwrap_or(authority).to_ascii_lowercase();
    ALLOWED_HOSTS.contains(&host.as_str())
}

/// `HttpsSource` for one channel's fixed-tag release (`update-<channel>`).
pub struct HttpsSource {
    agent: ureq::Agent,
    base: String,
}

impl HttpsSource {
    /// `https://github.com/moranmoa/MVP/releases/download/update-<channel>/`.
    pub fn release(channel: &str) -> Result<Self, SourceError> {
        if !is_valid_name(channel) {
            return Err(SourceError::Refused(format!("bad channel {channel:?}")));
        }
        Ok(HttpsSource { agent: agent(), base: format!("{RELEASE_BASE}update-{channel}/") })
    }

    fn url(&self, file: &str) -> String {
        format!("{}{file}", self.base)
    }

    /// GET with manual, checked redirects and a body cap.
    fn get(&self, url: &str, max_len: u64, out: &mut dyn Write) -> Result<(), SourceError> {
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            if !is_allowed_url(&url) {
                return Err(SourceError::Refused("redirect to a host that is not allowed".into()));
            }
            let resp = self.agent.get(&url).call().map_err(|e| match e {
                ureq::Error::StatusCode(404) => SourceError::NotFound,
                ureq::Error::StatusCode(code) => SourceError::Refused(format!("HTTP {code}")),
                other => SourceError::Offline(other.to_string()),
            })?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let Some(next) = resp.headers().get("location").and_then(|v| v.to_str().ok()) else {
                    return Err(SourceError::Refused("redirect without a location".into()));
                };
                url = next.to_string();
                continue;
            }
            match status {
                200 => {}
                404 => return Err(SourceError::NotFound),
                _ => return Err(SourceError::Refused(format!("HTTP {status}"))),
            }
            let mut body = resp.into_body().into_reader().take(max_len + 1);
            let n = std::io::copy(&mut body, out).map_err(|e| SourceError::Offline(e.to_string()))?;
            if n > max_len {
                return Err(SourceError::TooLarge);
            }
            return Ok(());
        }
        Err(SourceError::Refused("too many redirects".into()))
    }

    fn get_vec(&self, file: &str, max_len: usize) -> Result<Vec<u8>, SourceError> {
        let mut buf = Vec::new();
        self.get(&self.url(file), max_len as u64, &mut buf)?;
        Ok(buf)
    }
}

impl UpdateSource for HttpsSource {
    fn manifest(&self, _channel: &str) -> Result<Vec<u8>, SourceError> {
        // The channel is fixed by the release tag (`update-<channel>`).
        self.get_vec("manifest.json", MAX_MANIFEST_BYTES)
    }

    /// `root-2.json`, `root-3.json`, ... until the first missing one.
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError> {
        let mut files = Vec::new();
        let mut total = 0;
        for n in 2..2 + MAX_ROOT_UPDATES_PER_FETCH {
            match self.get_vec(&format!("root-{n}.json"), MAX_ROOT_UPDATE_BYTES) {
                Ok(f) => {
                    total += f.len();
                    if total > MAX_ROOT_FETCH_BYTES {
                        return Err(SourceError::TooLarge);
                    }
                    files.push(f);
                }
                Err(SourceError::NotFound) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(files)
    }

    fn artifact(&self, version: Version, platform: &str, max_len: u64, out: &mut dyn Write) -> Result<(), SourceError> {
        if !is_valid_name(platform) {
            return Err(SourceError::Refused(format!("bad platform {platform:?}")));
        }
        self.get(&self.url(&format!("mvs-{version}-{platform}.zip")), max_len, out)
    }
}

/// rustls with ring and the OS certificate store; redirects handled by
/// `get` (never automatically); no cookies; bounded timeouts.
fn agent() -> ureq::Agent {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(provider)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(0)
        .http_status_as_error(false)
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("mvs-updater/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_to_the_exact_github_hosts() {
        for ok in [
            "https://github.com/moranmoa/MVP/releases/download/update-alpha/manifest.json",
            "https://objects.githubusercontent.com/github-production-release-asset/1?x=y",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1",
            "https://GitHub.com:443/x",
        ] {
            assert!(is_allowed_url(ok), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://raw.githubusercontent.com/x",
            "https://user-images.githubusercontent.com/x",
            "https://evil.com/x",
            "https://github.com.evil.com/x",
            "https://evilgithub.com/x",
            "https://github.com@evil.com/x",
            "https://user@github.com/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
            "//github.com/x",
            "/relative",
        ] {
            assert!(!is_allowed_url(bad), "{bad}");
        }
    }

    #[test]
    fn release_urls_are_fixed_and_channel_checked() {
        let s = HttpsSource::release("alpha").unwrap();
        assert_eq!(s.url("manifest.json"), "https://github.com/moranmoa/MVP/releases/download/update-alpha/manifest.json");
        assert!(HttpsSource::release("../x").is_err());
        assert!(HttpsSource::release("Alpha").is_err());
    }

    /// Needs the network: `cargo test -p updater -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_manifest_fetch_answers() {
        let s = HttpsSource::release("alpha").unwrap();
        match s.manifest("alpha") {
            Ok(b) => assert!(b.len() <= MAX_MANIFEST_BYTES),
            Err(e) => assert!(matches!(e, SourceError::NotFound), "{e}"),
        }
    }

    /// A real release asset: GitHub answers with a redirect to its asset
    /// host, which must be on the allow-list; the body cap must hold.
    #[test]
    #[ignore]
    fn live_redirect_to_the_asset_host_is_followed_and_capped() {
        let s = HttpsSource { agent: agent(), base: format!("{RELEASE_BASE}v0.1.0-spike1/") };
        let mut head = Vec::new();
        assert_eq!(s.get(&s.url("spike-peer.exe"), 1024, &mut head), Err(SourceError::TooLarge));
        assert_eq!(&head[..2], b"MZ");
    }
}
