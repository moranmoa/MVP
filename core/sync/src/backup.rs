//! Server-list backups on the wire (G-91, restore phase A; design note
//! `docs/design/restore-servers-from-network.md`, #291). The blob is the
//! api's (`api::backup`, #317): opaque here except for its outer header,
//! which every holder checks before keeping it.
//!
//! ```text
//! "mvsb1" ‖ tag[32] ‖ w_pub[32] ‖ version u64le ‖ nonce[24] ‖ ciphertext ‖ sig[64]
//! ```
//!
//! `/mvs/backup/1`, one request, one answer:
//! - `0x01 ‖ blob`: keep it. Taken only from a verified current member of a
//!   server we share (in phase A: my own other devices), only if
//!   [`check`] passes, and only if its version is above the one held. The
//!   answer is one byte: 1 kept, 0 not (whatever the reason).
//! - `0x02 ‖ tag`: the blob held under exactly that tag, or the same empty
//!   answer for "none", "over the rate cap" or anything else (R3). Anyone
//!   may ask: only the owner of the words can compute a tag.
//!
//! A node holds its own backup and at most [`MAX_HELD`] others', at most one
//! per bound identity (R1), each for [`RETENTION`] after its last write,
//! and announces itself as a DHT provider of each tag it holds.
//! The relay mailbox (devops, `infra/relay`) speaks the same protocol.

use std::io;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, VerifyingKey};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, PeerId, StreamProtocol};

use crate::limits::RateLimiter;

pub(crate) const BACKUP_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/backup/1");
/// The largest blob (the api's `BACKUP_MAX`, design §2).
pub const MAX_BACKUP: usize = 64 * 1024;
/// Other people's backups one node holds (design §2, the buddy quota).
pub const MAX_HELD: usize = 8;
/// How long a held backup lives after its last write (buddies, design §7).
pub const RETENTION: Duration = Duration::from_secs(180 * 24 * 3600);
/// How often a holder re-announces itself as a provider (design §2).
pub const REANNOUNCE: Duration = Duration::from_secs(12 * 3600);

const MAGIC: &[u8; 5] = b"mvsb1";
const TAG_CONTEXT: &str = "mvs/backup-tag/v1";
const HEADER: usize = 5 + 32 + 32 + 8 + 24;
const SIG: usize = 64;
const TAG_LEN: usize = 32;
const PUT: u8 = 0x01;
const GET: u8 = 0x02;

/// Writes accepted per tag (design §2) and per peer, and fetches answered
/// per peer and overall, per hour.
pub(crate) const WRITES_PER_TAG: u32 = 12;
pub(crate) const WRITES_PER_PEER: u32 = 30;
pub(crate) const FETCHES_PER_PEER: u32 = 60;
pub(crate) const FETCHES_ALL: u32 = 600;
const MAX_TRACKED: usize = 1024;

/// What a holder checks before keeping a blob (the api's `backup::check`,
/// mirrored: sync doesn't depend on the api): the header, the tag is the
/// write key's, the size, and a strict Ed25519 signature by that key over
/// everything before it. The tag and version on success.
pub fn check(blob: &[u8]) -> Option<([u8; 32], u64)> {
    if blob.len() > MAX_BACKUP || blob.len() < HEADER + 16 + SIG || &blob[..5] != MAGIC {
        return None;
    }
    let tag: [u8; 32] = blob[5..37].try_into().ok()?;
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

/// One `/mvs/backup/1` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    Put(Vec<u8>),
    Get([u8; TAG_LEN]),
}

/// The codec: a kind byte then the body, to the end of the stream; reads
/// stop after the largest valid size, and anything longer fails the stream.
#[derive(Clone, Default)]
pub(crate) struct BackupCodec;

pub(crate) type BackupBehaviour = request_response::Behaviour<BackupCodec>;

pub(crate) fn behaviour() -> BackupBehaviour {
    request_response::Behaviour::new(
        [(BACKUP_PROTOCOL, request_response::ProtocolSupport::Full)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
    )
}

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
    /// Put: one byte (1 kept, 0 not). Get: the blob, or empty.
    type Response = Vec<u8>;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let bytes = read_capped(io, 1 + MAX_BACKUP).await?;
        match bytes.split_first() {
            Some((&PUT, blob)) if !blob.is_empty() => Ok(Request::Put(blob.to_vec())),
            Some((&GET, tag)) => {
                tag.try_into().map(Request::Get).map_err(|_| invalid("a backup tag is 32 bytes"))
            }
            _ => Err(invalid("unknown backup request")),
        }
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_capped(io, MAX_BACKUP).await
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        request: Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
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

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        answer: Vec<u8>,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        if answer.len() > MAX_BACKUP {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "backup answer too large"));
        }
        io.write_all(&answer).await
    }
}

/// Write and fetch caps, per hour.
pub(crate) struct BackupLimits {
    writes_tag: RateLimiter<[u8; 32]>,
    writes_peer: RateLimiter<PeerId>,
    fetches_peer: RateLimiter<PeerId>,
    fetches_all: RateLimiter<()>,
}

impl BackupLimits {
    pub(crate) fn new() -> Self {
        BackupLimits {
            writes_tag: RateLimiter::per_hour(WRITES_PER_TAG, WRITES_PER_TAG, MAX_TRACKED),
            writes_peer: RateLimiter::per_hour(WRITES_PER_PEER, WRITES_PER_PEER, MAX_TRACKED),
            fetches_peer: RateLimiter::per_hour(FETCHES_PER_PEER, FETCHES_PER_PEER, MAX_TRACKED),
            fetches_all: RateLimiter::per_hour(FETCHES_ALL, FETCHES_ALL, 1),
        }
    }

    pub(crate) fn allow_write(&mut self, peer: PeerId, tag: [u8; 32], now: Instant) -> bool {
        self.writes_peer.allow(peer, now) && self.writes_tag.allow(tag, now)
    }

    pub(crate) fn allow_fetch(&mut self, peer: PeerId, now: Instant) -> bool {
        self.fetches_peer.allow(peer, now) && self.fetches_all.allow((), now)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use libp2p::request_response::Codec as _;

    /// A blob as the api makes it (the ciphertext is opaque here).
    pub(crate) fn blob(seed: u8, version: u64, body: usize) -> Vec<u8> {
        let w = SigningKey::from_bytes(&[seed; 32]);
        let w_pub = w.verifying_key().to_bytes();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&blake3::derive_key(TAG_CONTEXT, &w_pub));
        out.extend_from_slice(&w_pub);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&[7; 24]);
        out.extend(std::iter::repeat_n(9u8, body));
        let sig = w.sign(&out).to_bytes();
        out.extend_from_slice(&sig);
        out
    }

    #[test]
    fn a_holder_checks_the_header_tag_size_and_signature() {
        let good = blob(1, 3, 4096 + 16);
        let (tag, version) = check(&good).unwrap();
        assert_eq!(version, 3);
        assert_eq!(&good[5..37], &tag);
        // Any byte changed: the signature or the tag no longer holds.
        for i in [0, 5, 40, 70, HEADER + 1, good.len() - 1] {
            let mut bad = good.clone();
            bad[i] ^= 1;
            assert!(check(&bad).is_none(), "byte {i}");
        }
        // A tag that isn't the write key's, even signed.
        let w = SigningKey::from_bytes(&[2; 32]);
        let mut wrong = good[..good.len() - SIG].to_vec();
        wrong[5..37].copy_from_slice(&[0; 32]);
        wrong[37..69].copy_from_slice(&w.verifying_key().to_bytes());
        let sig = w.sign(&wrong).to_bytes();
        wrong.extend_from_slice(&sig);
        assert!(check(&wrong).is_none());
        // Too small, too large.
        assert!(check(&good[..HEADER + SIG]).is_none());
        assert!(check(&blob(1, 3, MAX_BACKUP)).is_none());
    }

    #[test]
    fn requests_round_trip_and_are_capped() {
        futures::executor::block_on(async {
            let mut codec = BackupCodec;
            for request in [Request::Put(blob(1, 1, 100)), Request::Get([5; 32])] {
                let mut wire = futures::io::Cursor::new(Vec::new());
                codec.write_request(&BACKUP_PROTOCOL, &mut wire, request.clone()).await.unwrap();
                let got = codec
                    .read_request(
                        &BACKUP_PROTOCOL,
                        &mut futures::io::Cursor::new(wire.into_inner()),
                    )
                    .await;
                assert_eq!(got.unwrap(), request);
            }
            for bad in [
                vec![],
                vec![3, 1, 2],
                vec![GET; 32],
                vec![PUT],
                [vec![PUT], vec![0; MAX_BACKUP + 1]].concat(),
            ] {
                let got = codec
                    .read_request(&BACKUP_PROTOCOL, &mut futures::io::Cursor::new(bad.clone()))
                    .await;
                assert!(got.is_err(), "{} bytes", bad.len());
            }
            let big = vec![0; MAX_BACKUP + 1];
            let got =
                codec.read_response(&BACKUP_PROTOCOL, &mut futures::io::Cursor::new(big)).await;
            assert!(got.is_err());
        });
    }

    #[test]
    fn a_holder_keeps_the_newest_one_per_owner_and_a_bounded_number() {
        let mut store = crate::Store::in_memory().unwrap();
        let (tag, mine) = ([1; 32], blob(1, 2, 32));
        assert!(store.put_backup(&tag, 2, &mine, None, 100).unwrap());
        // Only a higher version replaces it.
        assert!(!store.put_backup(&tag, 2, b"same", None, 101).unwrap());
        assert!(!store.put_backup(&tag, 1, b"older", None, 101).unwrap());
        assert!(store.put_backup(&tag, 3, b"newer", None, 102).unwrap());
        assert_eq!(store.backup(&tag).unwrap().unwrap(), b"newer");
        // Others': one tag per owner (R1), a new one replaces their last.
        assert!(store.put_backup(&[2; 32], 1, b"a", Some(&[9; 32]), 110).unwrap());
        assert!(store.put_backup(&[3; 32], 1, b"b", Some(&[9; 32]), 111).unwrap());
        assert!(store.backup(&[2; 32]).unwrap().is_none());
        // At most MAX_HELD others', the least recently written dropped; mine stays.
        for i in 0..MAX_HELD as u8 {
            assert!(store
                .put_backup(&[10 + i; 32], 1, b"x", Some(&[20 + i; 32]), 120 + i as u64)
                .unwrap());
        }
        assert!(store.backup(&[3; 32]).unwrap().is_none(), "the oldest other went");
        assert_eq!(store.backup_tags().unwrap().len(), MAX_HELD + 1);
        // Expiry takes others' only.
        store.expire_backups(10_000, 100).unwrap();
        assert_eq!(store.backup_tags().unwrap(), [tag]);
    }

    #[test]
    fn writes_and_fetches_are_capped_per_hour() {
        let mut limits = BackupLimits::new();
        let (peer, now) = (PeerId::random(), Instant::now());
        assert_eq!(
            (0..20).filter(|_| limits.allow_write(peer, [1; 32], now)).count(),
            WRITES_PER_TAG as usize
        );
        assert_eq!(
            (0..100).filter(|_| limits.allow_fetch(peer, now)).count(),
            FETCHES_PER_PEER as usize
        );
    }
}
