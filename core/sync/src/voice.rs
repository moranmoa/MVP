//! Voice signalling transport (voice v1, piece A): `/mvs/voice/1`.
//!
//! One request carries one opaque blob (the api's signed envelope) of at
//! most [`MAX_VOICE_BLOB`] bytes; the answer is empty. Signals are
//! ephemeral: never stored, never gossiped, never forwarded. They go only
//! between verified current members, the same check as hole punching
//! (`punch`): the peer is bound to an identity (GAP-4) that our read gate
//! accepts on a server we share, so not a stranger and not kicked.
//!
//! - Sending ([`crate::Node::voice_send`]) needs a connected peer bound to
//!   the identity we send to, and that identity a member.
//! - Receiving: a request from a peer that isn't a member, or over its rate
//!   cap, or over the size cap, is dropped unanswered. What passes becomes
//!   [`crate::NodeEvent::VoiceSignal`] with the identity the peer is bound to.
//!
//! The rate caps keep a member's flood well below the api's replay cache
//! (4096 nonces per 240 s): per peer [`PER_PEER_RATE`]/s with a burst of
//! [`PER_PEER_BURST`], and all peers together [`ALL_RATE`]/s with a burst of
//! [`ALL_BURST`], so at most 10 × 240 + 80 = 2480 signals reach the api in
//! any 240 s.

use std::io;
use std::time::Instant;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, PeerId, StreamProtocol};

use crate::limits::RateLimiter;

pub(crate) const VOICE_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/voice/1");
/// Largest blob, in bytes (the api's `voice::MAX_BLOB`).
pub const MAX_VOICE_BLOB: usize = 16 * 1024;
/// Signals accepted per peer: a burst, then a steady rate per second.
pub(crate) const PER_PEER_BURST: u32 = 20;
pub(crate) const PER_PEER_RATE: u32 = 5;
/// Signals accepted from all peers together.
pub(crate) const ALL_BURST: u32 = 80;
pub(crate) const ALL_RATE: u32 = 10;
/// Per-peer buckets remembered.
const MAX_TRACKED: usize = 1024;

/// Why a signal wasn't sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceError {
    /// Over [`MAX_VOICE_BLOB`].
    TooLarge,
    /// No connected peer is bound to that identity as a current member
    /// (unknown, not connected, not a member, or kicked).
    NotMember,
    /// The node has stopped.
    Stopped,
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VoiceError::TooLarge => "voice signal too large",
            VoiceError::NotMember => "no connected member with that identity",
            VoiceError::Stopped => "the node has stopped",
        })
    }
}

impl std::error::Error for VoiceError {}

/// Per-peer and overall caps on accepted signals.
pub(crate) struct VoiceLimits {
    per_peer: RateLimiter<PeerId>,
    all: RateLimiter<()>,
}

impl VoiceLimits {
    pub(crate) fn new() -> Self {
        VoiceLimits {
            per_peer: RateLimiter::new(PER_PEER_BURST, PER_PEER_RATE, MAX_TRACKED),
            all: RateLimiter::new(ALL_BURST, ALL_RATE, 1),
        }
    }

    /// Spends one signal of `peer`'s budget and the overall one; false (and
    /// nothing spent overall) if `peer` is over its cap.
    pub(crate) fn allow(&mut self, peer: PeerId, now: Instant) -> bool {
        self.per_peer.allow(peer, now) && self.all.allow((), now)
    }
}

/// The wire format: the request is the blob itself, up to the end of the
/// stream; the answer is empty. Reading stops after [`MAX_VOICE_BLOB`] + 1
/// bytes, and a longer blob fails the stream (so it goes unanswered).
#[derive(Clone, Default)]
pub(crate) struct VoiceCodec;

pub(crate) type VoiceBehaviour = request_response::Behaviour<VoiceCodec>;

pub(crate) fn behaviour() -> VoiceBehaviour {
    request_response::Behaviour::new(
        [(VOICE_PROTOCOL, request_response::ProtocolSupport::Full)],
        request_response::Config::default().with_request_timeout(std::time::Duration::from_secs(10)),
    )
}

async fn read_blob<T: AsyncRead + Unpin + Send>(io: &mut T) -> io::Result<Vec<u8>> {
    let mut blob = Vec::new();
    io.take(MAX_VOICE_BLOB as u64 + 1).read_to_end(&mut blob).await?;
    if blob.len() > MAX_VOICE_BLOB {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "voice signal too large"));
    }
    Ok(blob)
}

#[async_trait::async_trait]
impl request_response::Codec for VoiceCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = ();

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_blob(io).await
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<()>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut rest = Vec::new();
        io.take(1).read_to_end(&mut rest).await?;
        Ok(())
    }

    async fn write_request<T>(&mut self, _: &StreamProtocol, io: &mut T, blob: Vec<u8>) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        if blob.len() > MAX_VOICE_BLOB {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "voice signal too large"));
        }
        io.write_all(&blob).await
    }

    async fn write_response<T>(&mut self, _: &StreamProtocol, _: &mut T, _: ()) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::request_response::Codec as _;
    use std::time::Duration;

    #[test]
    fn a_blob_over_16_kib_is_refused_on_read() {
        let protocol = VOICE_PROTOCOL;
        futures::executor::block_on(async {
            let ok = vec![7u8; MAX_VOICE_BLOB];
            let got = VoiceCodec.read_request(&protocol, &mut futures::io::Cursor::new(ok.clone())).await.unwrap();
            assert_eq!(got, ok);
            let big = vec![7u8; MAX_VOICE_BLOB + 1];
            assert!(VoiceCodec.read_request(&protocol, &mut futures::io::Cursor::new(big.clone())).await.is_err());
            // Nor does one go out.
            let mut sink = futures::io::Cursor::new(Vec::new());
            assert!(VoiceCodec.write_request(&protocol, &mut sink, big).await.is_err());
            assert!(sink.into_inner().is_empty());
        });
    }

    #[test]
    fn at_most_5_per_second_per_peer_after_a_burst_of_20() {
        let mut limits = VoiceLimits::new();
        let (peer, other) = (PeerId::random(), PeerId::random());
        let start = Instant::now();
        let burst = (0..30).filter(|_| limits.allow(peer, start)).count();
        assert_eq!(burst, PER_PEER_BURST as usize);
        // One second later: five more, no sixth.
        let later = start + Duration::from_secs(1);
        assert_eq!((0..10).filter(|_| limits.allow(peer, later)).count(), PER_PEER_RATE as usize);
        // Another peer has its own budget.
        assert!(limits.allow(other, later));
        // Over 240 s a peer gets at most the burst plus 5 a second.
        let mut limits = VoiceLimits::new();
        let mut accepted = 0;
        for tenth in 0..2400u64 {
            let now = start + Duration::from_millis(tenth * 100);
            accepted += (0..10).filter(|_| limits.allow(peer, now)).count();
        }
        assert!(accepted <= (PER_PEER_BURST + PER_PEER_RATE * 240) as usize, "{accepted}");
    }

    #[test]
    fn all_peers_together_stay_well_below_the_replay_cache() {
        let mut limits = VoiceLimits::new();
        let peers: Vec<PeerId> = (0..8).map(|_| PeerId::random()).collect();
        let start = Instant::now();
        let mut accepted = 0;
        for tenth in 0..2400u64 {
            let now = start + Duration::from_millis(tenth * 100);
            for peer in &peers {
                accepted += (0..5).filter(|_| limits.allow(*peer, now)).count();
            }
        }
        assert!(accepted <= (ALL_BURST + ALL_RATE * 240) as usize, "{accepted}");
        assert!(accepted < 4096 / 2 + 512, "{accepted} of the api's 4096 nonces");
    }
}
