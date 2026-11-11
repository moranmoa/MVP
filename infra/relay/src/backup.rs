//! The backup mailbox: `/mvs/backup/1` on the relay (restore phase A,
//! `docs/design/restore-servers-from-network.md` §2, #291; the protocol is
//! core/sync's `backup.rs`, #339).
//!
//! ```text
//! blob: "mvsb1" ‖ tag[32] ‖ w_pub[32] ‖ version u64le ‖ nonce[24] ‖ ciphertext ‖ sig[64]
//! 0x01 ‖ blob -> one byte: 1 kept, 0 not (whatever the reason)
//! 0x02 ‖ tag  -> the blob held under exactly that tag, or empty (R3)
//! ```
//!
//! What the relay keeps: only blobs signed by the tag's own write key `w`,
//! that is, written by the owner's own devices (phase A, #339's
//! `write_owner`). No buddy (phase B) blobs, no index of who wrote what.
//! Quotas, all constants (design §2), checked before anything is stored:
//! - per write: the header, `tag == derive_key("mvs/backup-tag/v1", w_pub)`,
//!   a strict Ed25519 signature by `w`, at most 64 KiB, the padded plaintext
//!   a multiple of 4 KiB, and a version above the one held;
//! - per tag: one blob, at most 12 writes an hour;
//! - per source: at most 30 writes an hour, 5 new tags a day, 60 fetches an
//!   hour. The source is the remote IP; for the reverse proxy's address
//!   (PROXY_IPS, every wss client) it is the peer ID instead;
//! - in total: at most 50,000 tags. When full, new tags are refused; a held
//!   backup is never evicted;
//! - expiry 13 months after the last write;
//! - fetch by exact tag only, with one empty answer for absent, expired,
//!   over a cap or anything else;
//! - logs: counts only, never a tag, an address or a peer ID.

use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    io::{self, Read},
    net::IpAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ed25519_dalek::{Signature, VerifyingKey};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, PeerId, StreamProtocol};

pub const BACKUP_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/backup/1");

pub const MAX_BACKUP: usize = 64 * 1024;
pub const PAD: usize = 4096;
pub const MAX_TAGS: usize = 50_000;
pub const RETENTION_SECS: u64 = 395 * 24 * 3600; // 13 months
pub const WRITES_PER_TAG_HOUR: u32 = 12;
pub const WRITES_PER_SOURCE_HOUR: u32 = 30;
pub const NEW_TAGS_PER_SOURCE_DAY: u32 = 5;
pub const FETCHES_PER_SOURCE_HOUR: u32 = 60;
/// Sources and tags tracked by the rate limiters; past it, new ones are
/// refused until windows expire (never a reason to let one through).
const MAX_TRACKED: usize = 16_384;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
pub const SWEEP_EVERY: Duration = Duration::from_secs(DAY);

const MAGIC: &[u8; 5] = b"mvsb1";
const TAG_CONTEXT: &str = "mvs/backup-tag/v1";
const HEADER: usize = 5 + 32 + 32 + 8 + 24;
const AEAD_TAG: usize = 16;
const SIG: usize = 64;
const PUT: u8 = 0x01;
const GET: u8 = 0x02;
/// On disk: last write (u64le seconds) then the blob.
const STAMP: usize = 8;

pub type Tag = [u8; 32];

/// The holder check (core/sync `backup::check`, plus the design's 4 KiB
/// step): the tag and version on success.
pub fn check(blob: &[u8]) -> Option<(Tag, u64)> {
    if blob.len() > MAX_BACKUP || blob.len() < HEADER + PAD + AEAD_TAG + SIG || &blob[..5] != MAGIC {
        return None;
    }
    if (blob.len() - HEADER - AEAD_TAG - SIG) % PAD != 0 {
        return None;
    }
    let tag: Tag = blob[5..37].try_into().ok()?;
    let w_pub: [u8; 32] = blob[37..69].try_into().ok()?;
    let version = u64::from_le_bytes(blob[69..77].try_into().ok()?);
    if blake3::derive_key(TAG_CONTEXT, &w_pub) != tag {
        return None;
    }
    let (signed, sig) = blob.split_at(blob.len() - SIG);
    let key = VerifyingKey::from_bytes(&w_pub).ok()?;
    key.verify_strict(signed, &Signature::from_bytes(sig.try_into().ok()?)).ok()?;
    Some((tag, version))
}

/// Who a request counts against.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    Ip(IpAddr),
    /// Behind the reverse proxy every client has the proxy's IP.
    Proxied(PeerId),
}

/// Fixed-window counters per key.
struct Windows<K> {
    span: u64,
    max: u32,
    seen: HashMap<K, (u64, u32)>,
}

impl<K: Eq + Hash + Copy> Windows<K> {
    fn new(span: u64, max: u32) -> Self {
        Windows { span, max, seen: HashMap::new() }
    }

    fn would_allow(&mut self, key: K, now: u64) -> bool {
        let window = now / self.span;
        match self.seen.get(&key) {
            Some(&(w, n)) if w == window => n < self.max,
            Some(_) => true,
            None => {
                if self.seen.len() >= MAX_TRACKED {
                    self.seen.retain(|_, (w, _)| *w == window);
                }
                self.seen.len() < MAX_TRACKED
            }
        }
    }

    fn take(&mut self, key: K, now: u64) {
        let window = now / self.span;
        let entry = self.seen.entry(key).or_insert((window, 0));
        if entry.0 != window {
            *entry = (window, 0);
        }
        entry.1 += 1;
    }
}

/// Why a write was not kept (for the counts in the stats line only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Invalid,
    NotNewer,
    RateTag,
    RateSource,
    NewTagsSource,
    Full,
    Disk,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Counts {
    pub stored: usize,
    pub writes_ok: u64,
    pub refused_invalid: u64,
    pub refused_not_newer: u64,
    pub refused_rate: u64,
    pub refused_full: u64,
    pub refused_disk: u64,
    pub fetch_hit: u64,
    pub fetch_miss: u64,
    pub expired: u64,
}

pub struct Mailbox {
    dir: PathBuf,
    /// tag -> (version, last write, unix seconds).
    held: HashMap<Tag, (u64, u64)>,
    writes_tag: Windows<Tag>,
    writes_source: Windows<Source>,
    new_tags_source: Windows<Source>,
    fetches_source: Windows<Source>,
    pub counts: Counts,
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hex(tag: &Tag) -> String {
    tag.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(name: &str) -> Option<Tag> {
    if name.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(name.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

impl Mailbox {
    /// Opens (or creates) the store and indexes it, reading only each file's
    /// stamp and header. Files that don't parse are removed.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut held = HashMap::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let parsed = unhex(&name).and_then(|tag| {
                let mut head = [0u8; STAMP + HEADER];
                fs::File::open(&path).ok()?.read_exact(&mut head).ok()?;
                let stamp = u64::from_le_bytes(head[..STAMP].try_into().ok()?);
                let blob = &head[STAMP..];
                (&blob[..5] == MAGIC && blob[5..37] == tag).then(|| {
                    (tag, u64::from_le_bytes(blob[69..77].try_into().unwrap()), stamp)
                })
            });
            match parsed {
                Some((tag, version, stamp)) if held.len() < MAX_TAGS => {
                    held.insert(tag, (version, stamp));
                }
                // A temp file from an interrupted write, or anything else.
                _ => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        let mut mailbox = Mailbox {
            dir: dir.to_path_buf(),
            held,
            writes_tag: Windows::new(HOUR, WRITES_PER_TAG_HOUR),
            writes_source: Windows::new(HOUR, WRITES_PER_SOURCE_HOUR),
            new_tags_source: Windows::new(DAY, NEW_TAGS_PER_SOURCE_DAY),
            fetches_source: Windows::new(HOUR, FETCHES_PER_SOURCE_HOUR),
            counts: Counts::default(),
        };
        mailbox.counts.stored = mailbox.held.len();
        Ok(mailbox)
    }

    pub fn tags(&self) -> impl Iterator<Item = &Tag> {
        self.held.keys()
    }

    fn path(&self, tag: &Tag) -> PathBuf {
        self.dir.join(hex(tag))
    }

    /// A write: every check first, then an atomic replace on disk. The
    /// stored tag on success (new or replaced).
    pub fn put(&mut self, source: Source, blob: &[u8], now: u64) -> Result<Tag, Refused> {
        let result = self.try_put(source, blob, now);
        let c = &mut self.counts;
        match result {
            Ok(_) => c.writes_ok += 1,
            Err(Refused::Invalid) => c.refused_invalid += 1,
            Err(Refused::NotNewer) => c.refused_not_newer += 1,
            Err(Refused::RateTag | Refused::RateSource | Refused::NewTagsSource) => c.refused_rate += 1,
            Err(Refused::Full) => c.refused_full += 1,
            Err(Refused::Disk) => c.refused_disk += 1,
        }
        c.stored = self.held.len();
        result
    }

    fn try_put(&mut self, source: Source, blob: &[u8], now: u64) -> Result<Tag, Refused> {
        if !self.writes_source.would_allow(source, now) {
            return Err(Refused::RateSource);
        }
        let (tag, version) = check(blob).ok_or(Refused::Invalid)?;
        let known = self.held.get(&tag).copied();
        if let Some((held, _)) = known {
            if version <= held {
                return Err(Refused::NotNewer);
            }
        }
        if !self.writes_tag.would_allow(tag, now) {
            return Err(Refused::RateTag);
        }
        if known.is_none() {
            if self.held.len() >= MAX_TAGS {
                return Err(Refused::Full);
            }
            if !self.new_tags_source.would_allow(source, now) {
                return Err(Refused::NewTagsSource);
            }
        }
        let path = self.path(&tag);
        let tmp = path.with_extension("tmp");
        let mut bytes = Vec::with_capacity(STAMP + blob.len());
        bytes.extend_from_slice(&now.to_le_bytes());
        bytes.extend_from_slice(blob);
        let written = fs::write(&tmp, &bytes)
            .and_then(|_| fs::File::open(&tmp)?.sync_all())
            .and_then(|_| fs::rename(&tmp, &path));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
            return Err(Refused::Disk);
        }
        self.writes_source.take(source, now);
        self.writes_tag.take(tag, now);
        if known.is_none() {
            self.new_tags_source.take(source, now);
        }
        self.held.insert(tag, (version, now));
        Ok(tag)
    }

    /// A fetch: the blob, or None for absent, expired, over the cap or a
    /// read error (the caller answers all of them the same way).
    pub fn get(&mut self, source: Source, tag: &Tag, now: u64) -> Option<Vec<u8>> {
        let found = self.try_get(source, tag, now);
        if found.is_some() {
            self.counts.fetch_hit += 1;
        } else {
            self.counts.fetch_miss += 1;
        }
        found
    }

    fn try_get(&mut self, source: Source, tag: &Tag, now: u64) -> Option<Vec<u8>> {
        if !self.fetches_source.would_allow(source, now) {
            return None;
        }
        self.fetches_source.take(source, now);
        let &(_, stamp) = self.held.get(tag)?;
        if now.saturating_sub(stamp) > RETENTION_SECS {
            return None;
        }
        let bytes = fs::read(self.path(tag)).ok()?;
        (bytes.len() > STAMP).then(|| bytes[STAMP..].to_vec())
    }

    /// Drops backups not written for [`RETENTION_SECS`]; the tags dropped.
    pub fn sweep(&mut self, now: u64) -> Vec<Tag> {
        let old: Vec<Tag> = self
            .held
            .iter()
            .filter(|(_, &(_, stamp))| now.saturating_sub(stamp) > RETENTION_SECS)
            .map(|(tag, _)| *tag)
            .collect();
        for tag in &old {
            let _ = fs::remove_file(self.path(tag));
            self.held.remove(tag);
        }
        self.counts.expired += old.len() as u64;
        self.counts.stored = self.held.len();
        old
    }
}

/// One `/mvs/backup/1` request (core/sync's codec, byte for byte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Put(Vec<u8>),
    Get(Tag),
}

#[derive(Clone, Default)]
pub struct BackupCodec;

async fn read_capped<T: AsyncRead + Unpin + Send>(io: &mut T, max: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io.take(max as u64 + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "backup message too large"));
    }
    Ok(bytes)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

#[async_trait::async_trait]
impl request_response::Codec for BackupCodec {
    type Protocol = StreamProtocol;
    type Request = Request;
    type Response = Vec<u8>;

    async fn read_request<T: AsyncRead + Unpin + Send>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Request> {
        let bytes = read_capped(io, 1 + MAX_BACKUP).await?;
        match bytes.split_first() {
            Some((&PUT, blob)) if !blob.is_empty() => Ok(Request::Put(blob.to_vec())),
            Some((&GET, tag)) => tag.try_into().map(Request::Get).map_err(|_| invalid("a backup tag is 32 bytes")),
            _ => Err(invalid("unknown backup request")),
        }
    }

    async fn read_response<T: AsyncRead + Unpin + Send>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>> {
        read_capped(io, MAX_BACKUP).await
    }

    async fn write_request<T: AsyncWrite + Unpin + Send>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        request: Request,
    ) -> io::Result<()> {
        match request {
            Request::Put(blob) if !blob.is_empty() && blob.len() <= MAX_BACKUP => {
                io.write_all(&[PUT]).await?;
                io.write_all(&blob).await
            }
            Request::Put(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "backup blob size")),
            Request::Get(tag) => {
                io.write_all(&[GET]).await?;
                io.write_all(&tag).await
            }
        }
    }

    async fn write_response<T: AsyncWrite + Unpin + Send>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        answer: Vec<u8>,
    ) -> io::Result<()> {
        if answer.len() > MAX_BACKUP {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "backup answer too large"));
        }
        io.write_all(&answer).await
    }
}

pub type BackupBehaviour = request_response::Behaviour<BackupCodec>;

pub fn behaviour() -> BackupBehaviour {
    request_response::Behaviour::new(
        [(BACKUP_PROTOCOL, request_response::ProtocolSupport::Inbound)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A blob as the api makes it: the plaintext padded to `pad` bytes.
    fn blob(seed: u8, version: u64, pad: usize) -> Vec<u8> {
        let w = SigningKey::from_bytes(&[seed; 32]);
        let w_pub = w.verifying_key().to_bytes();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&blake3::derive_key(TAG_CONTEXT, &w_pub));
        out.extend_from_slice(&w_pub);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&[7; 24]);
        out.extend(std::iter::repeat_n(9u8, pad + AEAD_TAG));
        let sig = w.sign(&out).to_bytes();
        out.extend_from_slice(&sig);
        out
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("relay-backup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn ip(n: u8) -> Source {
        Source::Ip(IpAddr::from([198, 51, 100, n]))
    }

    #[test]
    fn the_check_matches_sync_and_adds_the_4k_step() {
        let good = blob(1, 3, PAD);
        assert_eq!(check(&good).map(|(_, v)| v), Some(3));
        assert!(check(&blob(1, 3, 2 * PAD)).is_some());
        assert!(check(&blob(1, 3, 15 * PAD)).is_some(), "the largest: 101 + 61440 + 16 + 64 <= 64 KiB");
        assert!(check(&blob(1, 3, 16 * PAD)).is_none(), "over 64 KiB");
        assert!(check(&blob(1, 3, PAD + 1)).is_none(), "not a 4 KiB step");
        assert!(check(&blob(1, 3, 32)).is_none(), "below one step");
        for i in [0, 5, 40, 70, HEADER + 1, good.len() - 1] {
            let mut bad = good.clone();
            bad[i] ^= 1;
            assert!(check(&bad).is_none(), "byte {i}");
        }
        // Signed by a key whose tag it isn't.
        let w = SigningKey::from_bytes(&[2; 32]);
        let mut wrong = good[..good.len() - SIG].to_vec();
        wrong[37..69].copy_from_slice(&w.verifying_key().to_bytes());
        let sig = w.sign(&wrong).to_bytes();
        wrong.extend_from_slice(&sig);
        assert!(check(&wrong).is_none());
    }

    #[test]
    fn newer_versions_only_and_it_survives_a_restart() {
        let dir = tmp("versions");
        let mut m = Mailbox::open(&dir).unwrap();
        let (b2, b3) = (blob(1, 2, PAD), blob(1, 3, PAD));
        let tag = m.put(ip(1), &b2, 1000).unwrap();
        assert_eq!(m.put(ip(1), &b2, 1001), Err(Refused::NotNewer));
        assert_eq!(m.put(ip(1), &blob(1, 1, PAD), 1001), Err(Refused::NotNewer));
        assert_eq!(m.put(ip(1), &b3, 1002), Ok(tag));
        assert_eq!(m.get(ip(2), &tag, 1003).unwrap(), b3);
        drop(m);
        let mut m = Mailbox::open(&dir).unwrap();
        assert_eq!(m.counts.stored, 1);
        assert_eq!(m.get(ip(2), &tag, 1004).unwrap(), b3);
        assert_eq!(m.put(ip(1), &b3, 1005), Err(Refused::NotNewer), "the version came back from disk");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn per_tag_and_per_source_caps() {
        let dir = tmp("caps");
        let mut m = Mailbox::open(&dir).unwrap();
        let t0 = 10 * HOUR;
        // 12 writes an hour per tag, even from different sources.
        let ok = (1..=20).filter(|v| m.put(ip(*v as u8), &blob(1, *v, PAD), t0).is_ok()).count();
        assert_eq!(ok, WRITES_PER_TAG_HOUR as usize);
        assert!(m.put(ip(99), &blob(1, 100, PAD), t0 + HOUR).is_ok(), "next hour");
        // 5 new tags a day per source.
        let ok = (10..20).filter(|s| m.put(ip(50), &blob(*s, 1, PAD), t0).is_ok()).count();
        assert_eq!(ok, NEW_TAGS_PER_SOURCE_DAY as usize);
        // 30 writes an hour per source (updates of tags it already made).
        let mut m = Mailbox::open(&tmp("caps2")).unwrap();
        let mut ok = 0;
        for v in 1..=12 {
            for s in 30..33 {
                ok += m.put(ip(60), &blob(s, v, PAD), t0).is_ok() as usize;
            }
        }
        assert_eq!(ok, WRITES_PER_SOURCE_HOUR as usize);
        // 60 fetches an hour per source; then the same empty answer.
        let tag = *m.tags().next().unwrap();
        let hits = (0..100).filter(|_| m.get(ip(70), &tag, t0).is_some()).count();
        assert_eq!(hits, FETCHES_PER_SOURCE_HOUR as usize);
        assert!(m.get(ip(71), &tag, t0).is_some(), "another source isn't affected");
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(tmp("caps2")).ok();
    }

    #[test]
    fn proxied_clients_are_counted_per_peer() {
        let dir = tmp("proxied");
        let mut m = Mailbox::open(&dir).unwrap();
        let (a, b) = (Source::Proxied(PeerId::random()), Source::Proxied(PeerId::random()));
        for s in 0..NEW_TAGS_PER_SOURCE_DAY as u8 {
            m.put(a, &blob(100 + s, 1, PAD), 0).unwrap();
        }
        assert_eq!(m.put(a, &blob(120, 1, PAD), 0), Err(Refused::NewTagsSource));
        assert!(m.put(b, &blob(121, 1, PAD), 0).is_ok(), "another client behind the same proxy");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn full_refuses_new_tags_and_never_evicts() {
        let dir = tmp("full");
        let mut m = Mailbox::open(&dir).unwrap();
        let tag = m.put(ip(1), &blob(1, 1, PAD), 0).unwrap();
        // Fill the index to the cap with placeholders (no disk needed).
        for i in 0..(MAX_TAGS - 1) as u32 {
            let mut t = [0xee; 32];
            t[..4].copy_from_slice(&i.to_le_bytes());
            m.held.insert(t, (1, 0));
        }
        assert_eq!(m.put(ip(2), &blob(2, 1, PAD), 0), Err(Refused::Full));
        assert!(m.held.contains_key(&tag), "nothing was evicted");
        assert_eq!(m.put(ip(1), &blob(1, 2, PAD), 0), Ok(tag), "a held tag can still be updated");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expiry_after_13_months_and_one_answer_for_every_miss() {
        let dir = tmp("expiry");
        let mut m = Mailbox::open(&dir).unwrap();
        let tag = m.put(ip(1), &blob(1, 1, PAD), 0).unwrap();
        assert!(m.get(ip(2), &tag, RETENTION_SECS).is_some());
        assert!(m.get(ip(2), &tag, RETENTION_SECS + 1).is_none(), "expired, before the sweep");
        assert!(m.get(ip(2), &[0; 32], 1).is_none(), "never held");
        assert_eq!(m.sweep(RETENTION_SECS + 1), vec![tag]);
        assert!(!m.path(&tag).exists());
        assert_eq!(m.counts.stored, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn junk_files_are_dropped_on_open() {
        let dir = tmp("junk");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("not-a-tag"), b"x").unwrap();
        fs::write(dir.join(format!("{}.tmp", hex(&[1; 32]))), b"x").unwrap();
        fs::write(dir.join(hex(&[2; 32])), [0u8; 200]).unwrap();
        let m = Mailbox::open(&dir).unwrap();
        assert_eq!(m.counts.stored, 0);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
