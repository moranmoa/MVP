//! Invite envelopes on the wire (T8-02 piece 3,
//! `docs/design/short-invite-link.md` §4–§6). Publications are opaque here:
//! the api signs, verifies (C1) and decides what to serve; sync carries,
//! stores, caps and answers.
//!
//! - `/mvs/invite-pub/1`: a member pushes a publication to the server's
//!   other members. Request = the server's control handle (32 bytes) ‖ the
//!   publication; the answer is empty. Sent only to, and accepted only from,
//!   verified current members of that server (the voice/punch check).
//! - `/mvs/invite/1`: anyone, a non-member joiner included, asks for one
//!   envelope handle `h`. Request = `h` (exactly 32 bytes); answer = the
//!   publication served under exactly that `h`, or empty. "Not found",
//!   "over the rate cap" and "not in that server" all get the same empty
//!   answer (S2). No server secret is needed to ask or to answer.

use std::io;
use std::time::Instant;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, PeerId, StreamProtocol};

use crate::limits::RateLimiter;

pub(crate) const PUBLISH_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/invite-pub/1");
pub(crate) const ASK_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/invite/1");
/// Largest publication (the api's `crypto::invite::MAX_PUBLICATION` is 386).
pub const MAX_PUBLICATION: usize = 400;
/// Publications stored per server (C2).
pub const MAX_PER_SERVER: usize = 256;
const HANDLE_LEN: usize = 32;

/// S2: asks answered per peer (a burst, then per minute), and from all
/// peers together. Over the cap, the constant empty answer.
pub(crate) const ASK_PER_PEER: u32 = 10;
pub(crate) const ASK_ALL: u32 = 120;
/// Pushes accepted per member peer: a burst, then one a second (a member
/// re-announces up to [`MAX_PER_SERVER`] on connect).
pub(crate) const PUSH_BURST: u32 = MAX_PER_SERVER as u32;
pub(crate) const PUSH_RATE: u32 = 1;
const MAX_TRACKED: usize = 1024;

/// Caps on answered asks and accepted pushes.
pub(crate) struct InviteLimits {
    ask_peer: RateLimiter<PeerId>,
    ask_all: RateLimiter<()>,
    push_peer: RateLimiter<PeerId>,
}

impl InviteLimits {
    pub(crate) fn new() -> Self {
        InviteLimits {
            ask_peer: RateLimiter::per_minute(ASK_PER_PEER, ASK_PER_PEER, MAX_TRACKED),
            ask_all: RateLimiter::per_minute(ASK_ALL, ASK_ALL, 1),
            push_peer: RateLimiter::new(PUSH_BURST, PUSH_RATE, MAX_TRACKED),
        }
    }

    pub(crate) fn allow_ask(&mut self, peer: PeerId, now: Instant) -> bool {
        self.ask_peer.allow(peer, now) && self.ask_all.allow((), now)
    }

    pub(crate) fn allow_push(&mut self, peer: PeerId, now: Instant) -> bool {
        self.push_peer.allow(peer, now)
    }
}

/// Reads at most `max` bytes to the end of the stream; more fails it.
async fn read_capped<T: AsyncRead + Unpin + Send>(io: &mut T, max: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io.take(max as u64 + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invite message too large"));
    }
    Ok(bytes)
}

/// `/mvs/invite-pub/1`: control handle ‖ publication in; nothing back.
#[derive(Clone, Default)]
pub(crate) struct PublishCodec;

/// `/mvs/invite/1`: `h` in; a publication or nothing back.
#[derive(Clone, Default)]
pub(crate) struct AskCodec;

pub(crate) type PublishBehaviour = request_response::Behaviour<PublishCodec>;
pub(crate) type AskBehaviour = request_response::Behaviour<AskCodec>;

fn config() -> request_response::Config {
    request_response::Config::default().with_request_timeout(std::time::Duration::from_secs(10))
}

pub(crate) fn publish_behaviour() -> PublishBehaviour {
    request_response::Behaviour::new(
        [(PUBLISH_PROTOCOL, request_response::ProtocolSupport::Full)],
        config(),
    )
}

pub(crate) fn ask_behaviour() -> AskBehaviour {
    request_response::Behaviour::new(
        [(ASK_PROTOCOL, request_response::ProtocolSupport::Full)],
        config(),
    )
}

/// A push: which server (by its control handle) and the publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Push {
    pub control: [u8; HANDLE_LEN],
    pub publication: Vec<u8>,
}

#[async_trait::async_trait]
impl request_response::Codec for PublishCodec {
    type Protocol = StreamProtocol;
    type Request = Push;
    type Response = ();

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Push>
    where
        T: AsyncRead + Unpin + Send,
    {
        let bytes = read_capped(io, HANDLE_LEN + MAX_PUBLICATION).await?;
        if bytes.len() <= HANDLE_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty invite publication"));
        }
        let (control, publication) = bytes.split_at(HANDLE_LEN);
        Ok(Push {
            control: control.try_into().expect("32 bytes"),
            publication: publication.to_vec(),
        })
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<()>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_capped(io, 0).await.map(|_| ())
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        push: Push,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        if push.publication.is_empty() || push.publication.len() > MAX_PUBLICATION {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invite publication size"));
        }
        io.write_all(&push.control).await?;
        io.write_all(&push.publication).await
    }

    async fn write_response<T>(&mut self, _: &StreamProtocol, _: &mut T, _: ()) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        Ok(())
    }
}

#[async_trait::async_trait]
impl request_response::Codec for AskCodec {
    type Protocol = StreamProtocol;
    type Request = [u8; HANDLE_LEN];
    /// Empty: not found (or not answered, S2).
    type Response = Vec<u8>;

    async fn read_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
    ) -> io::Result<[u8; HANDLE_LEN]>
    where
        T: AsyncRead + Unpin + Send,
    {
        let bytes = read_capped(io, HANDLE_LEN).await?;
        bytes
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "an invite handle is 32 bytes"))
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_capped(io, MAX_PUBLICATION).await
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        handle: [u8; HANDLE_LEN],
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        io.write_all(&handle).await
    }

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        publication: Vec<u8>,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        if publication.len() > MAX_PUBLICATION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invite publication too large",
            ));
        }
        io.write_all(&publication).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::request_response::Codec as _;
    use std::time::Duration;

    fn read<C: request_response::Codec<Protocol = StreamProtocol>>(
        codec: &mut C,
        protocol: StreamProtocol,
        bytes: Vec<u8>,
    ) -> io::Result<C::Request> {
        futures::executor::block_on(
            codec.read_request(&protocol, &mut futures::io::Cursor::new(bytes)),
        )
    }

    #[test]
    fn an_ask_is_exactly_one_handle() {
        assert_eq!(read(&mut AskCodec, ASK_PROTOCOL, vec![7; 32]).unwrap(), [7; 32]);
        for len in [0, 31, 33, 1000] {
            assert!(read(&mut AskCodec, ASK_PROTOCOL, vec![7; len]).is_err(), "{len} bytes");
        }
    }

    #[test]
    fn answers_and_pushes_are_capped() {
        futures::executor::block_on(async {
            let mut codec = AskCodec;
            let ok = vec![1u8; MAX_PUBLICATION];
            let got = codec
                .read_response(&ASK_PROTOCOL, &mut futures::io::Cursor::new(ok.clone()))
                .await
                .unwrap();
            assert_eq!(got, ok);
            let big = vec![1u8; MAX_PUBLICATION + 1];
            assert!(codec
                .read_response(&ASK_PROTOCOL, &mut futures::io::Cursor::new(big.clone()))
                .await
                .is_err());
            let mut sink = futures::io::Cursor::new(Vec::new());
            assert!(codec.write_response(&ASK_PROTOCOL, &mut sink, big).await.is_err());
            // The constant "not found".
            let empty = codec
                .read_response(&ASK_PROTOCOL, &mut futures::io::Cursor::new(Vec::new()))
                .await
                .unwrap();
            assert!(empty.is_empty());

            let mut codec = PublishCodec;
            let push = Push { control: [3; 32], publication: vec![4; MAX_PUBLICATION] };
            let mut wire = futures::io::Cursor::new(Vec::new());
            codec.write_request(&PUBLISH_PROTOCOL, &mut wire, push.clone()).await.unwrap();
            let cursor = |bytes: Vec<u8>| futures::io::Cursor::new(bytes);
            let got = codec
                .read_request(&PUBLISH_PROTOCOL, &mut cursor(wire.into_inner()))
                .await
                .unwrap();
            assert_eq!(got, push);
            let big = cursor(vec![4; 32 + MAX_PUBLICATION + 1]);
            assert!(codec.read_request(&PUBLISH_PROTOCOL, &mut { big }).await.is_err());
            let bare = cursor(vec![4; 32]);
            assert!(
                codec.read_request(&PUBLISH_PROTOCOL, &mut { bare }).await.is_err(),
                "no publication"
            );
            let mut sink = futures::io::Cursor::new(Vec::new());
            let empty = Push { control: [3; 32], publication: vec![] };
            assert!(codec.write_request(&PUBLISH_PROTOCOL, &mut sink, empty).await.is_err());
        });
    }

    #[test]
    fn asks_are_capped_at_10_a_minute_per_peer_and_120_overall() {
        let mut limits = InviteLimits::new();
        let (peer, other) = (PeerId::random(), PeerId::random());
        let start = Instant::now();
        assert_eq!(
            (0..30).filter(|_| limits.allow_ask(peer, start)).count(),
            ASK_PER_PEER as usize
        );
        // Six seconds later: one more (10 a minute).
        let later = start + Duration::from_secs(6);
        assert_eq!((0..5).filter(|_| limits.allow_ask(peer, later)).count(), 1);
        assert!(limits.allow_ask(other, later));
        // Many peers together: at most 120 at once, then 2 a second.
        let mut limits = InviteLimits::new();
        let peers: Vec<PeerId> = (0..50).map(|_| PeerId::random()).collect();
        let burst = peers
            .iter()
            .map(|p| (0..10).filter(|_| limits.allow_ask(*p, start)).count())
            .sum::<usize>();
        assert_eq!(burst, ASK_ALL as usize);
    }
}
