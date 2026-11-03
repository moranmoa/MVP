//! Per-channel epoch keys.
//!
//! A channel's messages are encrypted under the key of the current *epoch*.
//! A new epoch starts with a rotation: a moderator draws a fresh random key
//! and wraps it separately for every member that may read the channel. A
//! removed member gets no wrap, so they can't read anything sent in the new
//! epoch. See README.md for the threat model.
//!
//! Crypto, in one place:
//! - A member's X25519 key is derived from their Ed25519 identity (the
//!   birational map, as libsodium's `crypto_sign_ed25519_pk_to_curve25519`
//!   does). No extra key has to be published before someone can be added.
//! - Wrap: one ephemeral X25519 key per rotation. For each recipient the
//!   shared secret goes through BLAKE3 `derive_key`, bound to the recipient,
//!   the ephemeral key and the whole rotation header; the 32-byte epoch key is
//!   then sealed with XChaCha20-Poly1305 under that one-time key.
//! - Messages: XChaCha20-Poly1305 with a random 24-byte nonce under a key
//!   derived from the epoch key. The associated data binds server, channel,
//!   epoch, rotation and author.
//!
//! A rotation is split into 1 to 32 parts, each the body of its own signed
//! `KeyRotation` record whose author is the rotator. Part body (all integers
//! little-endian):
//!
//! ```text
//! rotation   [u8; 32]   rotation ID (records' part header, 34 bytes in all)
//! part       u8         index, < parts
//! parts      u8         1..=32
//! ver        u8         must be VERSION        \
//! channel    [u8; 32]                           |
//! epoch      u64        0 for the first rotation of a channel
//! prev       u8 tag     0 = none (epoch 0 only), 1 = followed by the
//!                       rotation ID this one replaces (its epoch + 1)
//! ephemeral  [u8; 32]   X25519 public key      / header: identical in every part
//! count      u16        1..=WRAPS_PER_PART; every part but the last is full
//! wraps      count * (recipient [u8; 32] Ed25519 key, wrapped [u8; 48]),
//!            recipients strictly ascending, also across parts
//! ```
//!
//! rotation ID = BLAKE3 `derive_key("mvs/groupkeys v1 rotation id",
//! rotator ‖ server ‖ header)`. The header names only the channel, so the
//! server is bound here. A part whose ID doesn't match is refused, so nobody
//! but the rotator can add a part to a rotation.
//!
//! A rotator that retries sends the same signed records again. It must never
//! re-sign a part: a second record for the same part is equivocation, and the
//! whole rotation is refused.
//!
//! Message ciphertext (the encrypted body of a channel message record):
//!
//! ```text
//! ver        u8
//! epoch      u64
//! key hint   [u8; 8]    first 8 bytes of the rotation ID
//! nonce      [u8; 24]
//! sealed     plaintext + 16-byte Poly1305 tag
//! ```

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::{CryptoRng, RngCore};
use records::{ChannelId, Kind, PublicKey, RecordId, ServerId};
use zeroize::Zeroizing;

mod channel;
pub use channel::{
    Applied, ChannelKeys, Decrypted, RotationNeed, MAX_UNACCEPTED, MAX_UNACCEPTED_BYTES,
};

/// Format version of rotation bodies and message ciphertexts.
pub const VERSION: u8 = 1;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
pub const KEY_HINT_LEN: usize = 8;
/// An epoch key sealed for one recipient.
pub const WRAPPED_KEY_LEN: usize = KEY_LEN + TAG_LEN;
/// One recipient entry in a rotation: their key plus the sealed epoch key.
pub const WRAP_LEN: usize = 32 + WRAPPED_KEY_LEN;
/// Rotation body bytes before the wraps, when it has a `prev` (every epoch
/// after the first).
pub const ROTATION_HEADER_LEN: usize = 1 + 32 + 8 + 1 + 32 + 32 + 2;
/// Bytes a message ciphertext adds to its plaintext.
pub const MESSAGE_OVERHEAD: usize = 1 + 8 + KEY_HINT_LEN + NONCE_LEN + TAG_LEN;
/// Largest plaintext that still fits a channel message record once sealed.
pub const MAX_MESSAGE_PLAINTEXT: usize = MESSAGE_MAX_BODY - MESSAGE_OVERHEAD;
/// `records::Kind::Message.max_body()`, which isn't a const fn. A test keeps
/// the two equal.
const MESSAGE_MAX_BODY: usize = 16 * 1024;

/// Bytes `records` puts before every key rotation part: rotation ID, part
/// index, part count.
pub const PART_HEADER_LEN: usize = records::ROTATION_HEADER_LEN;
/// `records::Kind::KeyRotation.max_body()`, which isn't a const fn. A test
/// keeps the two equal.
const PART_MAX_BODY: usize = 16 * 1024;
/// Most parts one rotation is split into.
pub const MAX_PARTS: usize = records::MAX_ROTATION_PARTS as usize;
/// Wraps per part. Every part but the last carries exactly this many.
pub const WRAPS_PER_PART: usize =
    (PART_MAX_BODY - PART_HEADER_LEN - ROTATION_HEADER_LEN) / WRAP_LEN;
/// Most recipients one rotation can name: every part full.
pub const MAX_RECIPIENTS: usize = MAX_PARTS * WRAPS_PER_PART;
/// The most a complete rotation can take, all parts together.
pub const MAX_ROTATION_BYTES: usize = MAX_PARTS * PART_MAX_BODY;

// Domain separation strings. Never reuse one for another purpose.
const WRAP_AD: &[u8] = b"mvs/groupkeys/wrap/v1\0";
const WRAP_KDF: &str = "mvs/groupkeys v1 2026-11 wrap key";
const MESSAGE_AD: &[u8] = b"mvs/groupkeys/message/v1\0";
const MESSAGE_KDF: &str = "mvs/groupkeys v1 2026-11 message key";
const ROTATION_ID_KDF: &str = "mvs/groupkeys v1 rotation id";

/// How many parts a rotation for `recipients` members takes.
pub const fn parts_for(recipients: usize) -> usize {
    recipients.div_ceil(WRAPS_PER_PART)
}

/// Bytes of all part bodies of a rotation with `recipients` wraps (and a
/// `prev`).
pub const fn rotation_body_len(recipients: usize) -> usize {
    parts_for(recipients) * (PART_HEADER_LEN + ROTATION_HEADER_LEN) + recipients * WRAP_LEN
}

/// The ID of the rotation `rotator` made in `server` with `header` (ver to
/// ephemeral, as encoded). Every part carries it; `prev` and message key
/// hints name it. Binding the rotator means nobody else's part can claim the
/// rotation; binding the server (the header names only the channel) keeps IDs
/// apart across servers.
pub fn rotation_id(server: ServerId, rotator: PublicKey, header: &[u8]) -> RecordId {
    let mut material = Vec::with_capacity(2 * 32 + header.len());
    material.extend_from_slice(rotator.as_bytes());
    material.extend_from_slice(server.as_bytes());
    material.extend_from_slice(header);
    RecordId::from_bytes(blake3::derive_key(ROTATION_ID_KDF, &material))
}

/// The body limit of a records kind; checks the copy above stays in sync.
pub fn message_body_limit() -> usize {
    let limit = Kind::Message.max_body();
    debug_assert_eq!(limit, MESSAGE_MAX_BODY);
    limit
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    TrailingBytes,
    UnsupportedVersion(u8),
    /// The rotation names a different channel than the one being read.
    WrongChannel,
    /// Epoch 0 must have no `prev`, every later epoch must have one.
    BadEpochLink,
    EpochOverflow,
    /// Recipients not strictly ascending (unsorted or duplicated).
    NotCanonical,
    NoRecipients,
    TooManyRecipients(usize),
    /// A recipient key isn't a usable Ed25519 point.
    BadMemberKey(PublicKey),
    /// A part header out of range: no parts, too many, or index past the
    /// count.
    BadPart,
    /// A part's rotation ID isn't the one its rotator and header give.
    RotationIdMismatch,
    /// Not every part of the rotation is there.
    MissingParts,
    /// Parts that claim one rotation but don't fit together (index, count,
    /// rotator or header differ).
    PartMismatch,
    /// The ephemeral key gives an all-zero shared secret (small order).
    BadEphemeralKey,
    /// The sealed epoch key for us didn't open.
    UnwrapFailed,
    /// We hold no key for the epoch (never got a wrap for it, or the
    /// channel has no rotation yet).
    NoKey,
    /// The current key is readable by people no longer allowed in the
    /// channel; a moderator has to rotate before anyone sends.
    RotationRequired {
        removed: Vec<PublicKey>,
    },
    MessageTooLarge {
        len: usize,
        max: usize,
    },
    /// Authentication failed: wrong key, tampered ciphertext or associated
    /// data (server, channel, epoch, author).
    DecryptFailed,
    /// The message author wasn't a recipient of the epoch it claims.
    AuthorNotRecipient,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => write!(f, "truncated"),
            Error::TrailingBytes => write!(f, "trailing bytes"),
            Error::UnsupportedVersion(ver) => write!(f, "unsupported version {ver}"),
            Error::WrongChannel => write!(f, "rotation is for another channel"),
            Error::BadEpochLink => write!(f, "epoch and prev don't match"),
            Error::EpochOverflow => write!(f, "epoch counter overflow"),
            Error::NotCanonical => write!(f, "recipients are not strictly ascending"),
            Error::NoRecipients => write!(f, "rotation has no recipients"),
            Error::TooManyRecipients(n) => {
                write!(f, "{n} recipients, limit is {MAX_RECIPIENTS}")
            }
            Error::BadMemberKey(key) => write!(f, "{key:?} is not a usable member key"),
            Error::BadPart => write!(f, "rotation part header out of range"),
            Error::RotationIdMismatch => write!(f, "rotation ID does not match rotator and header"),
            Error::MissingParts => write!(f, "rotation is missing parts"),
            Error::PartMismatch => write!(f, "rotation parts do not fit together"),
            Error::BadEphemeralKey => write!(f, "ephemeral key has small order"),
            Error::UnwrapFailed => write!(f, "our wrapped key does not open"),
            Error::NoKey => write!(f, "no key for this epoch"),
            Error::RotationRequired { removed } => {
                write!(
                    f,
                    "key rotation required: {} removed member(s) still hold it",
                    removed.len()
                )
            }
            Error::MessageTooLarge { len, max } => {
                write!(f, "message is {len} bytes, limit is {max}")
            }
            Error::DecryptFailed => write!(f, "decryption failed"),
            Error::AuthorNotRecipient => write!(f, "author did not hold the epoch key"),
        }
    }
}

impl std::error::Error for Error {}

/// A 32-byte symmetric epoch key. Zeroed on drop; never printed.
#[derive(Clone)]
pub struct EpochKey(Zeroizing<[u8; KEY_LEN]>);

impl EpochKey {
    pub fn generate(rng: &mut (impl RngCore + CryptoRng)) -> EpochKey {
        let mut key = Zeroizing::new([0; KEY_LEN]);
        rng.fill_bytes(key.as_mut());
        EpochKey(key)
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> EpochKey {
        EpochKey(Zeroizing::new(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    fn message_cipher(&self) -> XChaCha20Poly1305 {
        let key = Zeroizing::new(blake3::derive_key(MESSAGE_KDF, self.0.as_ref()));
        XChaCha20Poly1305::new(key.as_ref().into())
    }
}

impl fmt::Debug for EpochKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EpochKey(..)")
    }
}

/// The X25519 public key of an Ed25519 identity. Rejects keys that don't
/// decode and small-order ("weak") keys.
pub fn x25519_public(identity: &PublicKey) -> Result<MontgomeryPoint, Error> {
    let key = VerifyingKey::from_bytes(identity.as_bytes())
        .map_err(|_| Error::BadMemberKey(*identity))?;
    if key.is_weak() {
        return Err(Error::BadMemberKey(*identity));
    }
    // ed25519-dalek 2 is built on curve25519-dalek 4; the point crosses to
    // our curve25519-dalek 5 as its canonical 32-byte encoding.
    Ok(MontgomeryPoint(key.to_montgomery().to_bytes()))
}

/// The X25519 secret matching [`x25519_public`]: the Ed25519 secret scalar,
/// which X25519 clamps the same way.
fn x25519_secret(identity: &SigningKey) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(identity.to_scalar_bytes())
}

/// The X25519 secret of one identity, derived once. A client builds one and
/// shares it (behind an `Arc`) between all its [`ChannelKeys`], so the
/// identity's signing key isn't copied into every channel. Zeroized on drop.
pub struct MemberSecret {
    public: PublicKey,
    secret: Zeroizing<[u8; 32]>,
}

impl MemberSecret {
    pub fn new(identity: &SigningKey) -> MemberSecret {
        MemberSecret {
            public: PublicKey::from_bytes(identity.verifying_key().to_bytes()),
            secret: x25519_secret(identity),
        }
    }

    /// The identity this secret belongs to.
    pub fn public(&self) -> PublicKey {
        self.public
    }
}

impl fmt::Debug for MemberSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemberSecret({:?})", self.public)
    }
}

/// Shared secret, rejecting the all-zero output of a small-order point.
fn diffie_hellman(public: MontgomeryPoint, secret: &[u8; 32]) -> Option<Zeroizing<[u8; 32]>> {
    let shared = Zeroizing::new(public.mul_clamped(*secret).to_bytes());
    // Not secret-dependent in a useful way: an all-zero result only happens
    // for a malformed public key.
    if shared.iter().all(|byte| *byte == 0) {
        None
    } else {
        Some(shared)
    }
}

/// Who a message is from and where it belongs. All of it is authenticated,
/// so a ciphertext only opens in exactly this context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageContext {
    pub server: ServerId,
    pub channel: ChannelId,
    pub epoch: u64,
    /// The rotation that set the epoch key.
    pub rotation: RecordId,
    pub author: PublicKey,
}

fn message_header(epoch: u64, rotation: &RecordId, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 + KEY_HINT_LEN + NONCE_LEN);
    out.push(VERSION);
    out.extend_from_slice(&epoch.to_le_bytes());
    out.extend_from_slice(&rotation.as_bytes()[..KEY_HINT_LEN]);
    out.extend_from_slice(nonce);
    out
}

fn message_ad(ctx: &MessageContext, header: &[u8]) -> Vec<u8> {
    let mut ad = Vec::with_capacity(MESSAGE_AD.len() + 4 * 32 + 8 + header.len());
    ad.extend_from_slice(MESSAGE_AD);
    ad.extend_from_slice(ctx.server.as_bytes());
    ad.extend_from_slice(ctx.channel.as_bytes());
    ad.extend_from_slice(&ctx.epoch.to_le_bytes());
    ad.extend_from_slice(ctx.rotation.as_bytes());
    ad.extend_from_slice(ctx.author.as_bytes());
    ad.extend_from_slice(header);
    ad
}

/// Encrypts a message body under `key` for exactly `ctx`.
pub fn seal(
    key: &EpochKey,
    ctx: &MessageContext,
    plaintext: &[u8],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Vec<u8>, Error> {
    if plaintext.len() > MAX_MESSAGE_PLAINTEXT {
        return Err(Error::MessageTooLarge { len: plaintext.len(), max: MAX_MESSAGE_PLAINTEXT });
    }
    let mut nonce = [0; NONCE_LEN];
    rng.fill_bytes(&mut nonce);
    let header = message_header(ctx.epoch, &ctx.rotation, &nonce);
    let ad = message_ad(ctx, &header);
    let sealed = key
        .message_cipher()
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &ad })
        .expect("XChaCha20-Poly1305 encryption of a bounded message can't fail");
    let mut out = header;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// The routing part of a ciphertext: which epoch key it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageHeader {
    pub epoch: u64,
    pub key_hint: [u8; KEY_HINT_LEN],
}

impl MessageHeader {
    pub fn parse(ciphertext: &[u8]) -> Result<MessageHeader, Error> {
        let mut reader = Reader { bytes: ciphertext };
        let ver = reader.u8()?;
        if ver != VERSION {
            return Err(Error::UnsupportedVersion(ver));
        }
        let epoch = reader.u64()?;
        let key_hint = reader.array()?;
        reader.take(NONCE_LEN + TAG_LEN)?;
        Ok(MessageHeader { epoch, key_hint })
    }

    /// Whether this ciphertext names `rotation`'s key.
    pub fn matches(&self, rotation: &RecordId) -> bool {
        rotation.as_bytes()[..KEY_HINT_LEN] == self.key_hint
    }
}

/// Decrypts a ciphertext from [`seal`]. Fails unless `key` and every field
/// of `ctx` are the ones it was sealed with.
pub fn open(key: &EpochKey, ctx: &MessageContext, ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
    let header = MessageHeader::parse(ciphertext)?;
    if header.epoch != ctx.epoch || !header.matches(&ctx.rotation) {
        return Err(Error::DecryptFailed);
    }
    if ciphertext.len() > message_body_limit() {
        return Err(Error::MessageTooLarge {
            len: ciphertext.len() - MESSAGE_OVERHEAD,
            max: MAX_MESSAGE_PLAINTEXT,
        });
    }
    let header_len = MESSAGE_OVERHEAD - TAG_LEN;
    let (header_bytes, sealed) = ciphertext.split_at(header_len);
    let nonce = &header_bytes[header_len - NONCE_LEN..];
    let ad = message_ad(ctx, header_bytes);
    key.message_cipher()
        .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &ad })
        .map_err(|_| Error::DecryptFailed)
}

/// One part of a rotation, decoded from the body of one signed
/// `KeyRotation` record. It names its rotation, but holding one says
/// nothing about the other parts; see [`Rotation::assemble`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    rotation: RecordId,
    index: u8,
    parts: u8,
    rotator: PublicKey,
    channel: ChannelId,
    epoch: u64,
    prev: Option<RecordId>,
    ephemeral: MontgomeryPoint,
    /// ver..ephemeral, exactly as encoded. The same in every part.
    header: Vec<u8>,
    wraps: Vec<(PublicKey, [u8; WRAPPED_KEY_LEN])>,
}

impl Part {
    /// Parses a part body. `server` and `rotator` are the server and author of
    /// the signed record that carried it. Refuses a part whose rotation ID
    /// isn't the one its server, rotator and header give.
    pub fn decode(server: ServerId, rotator: PublicKey, body: &[u8]) -> Result<Part, Error> {
        let mut reader = Reader { bytes: body };
        let rotation = RecordId::from_bytes(reader.array()?);
        let index = reader.u8()?;
        let parts = reader.u8()?;
        if parts == 0 || usize::from(parts) > MAX_PARTS || index >= parts {
            return Err(Error::BadPart);
        }
        let ver = reader.u8()?;
        if ver != VERSION {
            return Err(Error::UnsupportedVersion(ver));
        }
        let channel = ChannelId::from_bytes(reader.array()?);
        let epoch = reader.u64()?;
        let prev = match reader.u8()? {
            0 => None,
            1 => Some(RecordId::from_bytes(reader.array()?)),
            _ => return Err(Error::BadEpochLink),
        };
        if (epoch == 0) != prev.is_none() {
            return Err(Error::BadEpochLink);
        }
        let ephemeral = MontgomeryPoint(reader.array()?);
        let header = body[PART_HEADER_LEN..body.len() - reader.bytes.len()].to_vec();
        if rotation_id(server, rotator, &header) != rotation {
            return Err(Error::RotationIdMismatch);
        }
        let count = usize::from(reader.u16()?);
        if count == 0 {
            return Err(Error::NoRecipients);
        }
        if count > WRAPS_PER_PART {
            return Err(Error::TooManyRecipients(count));
        }
        // Check the count against the bytes actually there before allocating.
        let wraps_len = count * WRAP_LEN;
        if reader.bytes.len() < wraps_len {
            return Err(Error::Truncated);
        }
        if reader.bytes.len() > wraps_len {
            return Err(Error::TrailingBytes);
        }
        let mut wraps: Vec<(PublicKey, [u8; WRAPPED_KEY_LEN])> = Vec::with_capacity(count);
        for _ in 0..count {
            let recipient = PublicKey::from_bytes(reader.array()?);
            if wraps.last().is_some_and(|(last, _)| *last >= recipient) {
                return Err(Error::NotCanonical);
            }
            wraps.push((recipient, reader.array()?));
        }
        Ok(Part { rotation, index, parts, rotator, channel, epoch, prev, ephemeral, header, wraps })
    }

    /// The rotation this part belongs to.
    pub fn rotation(&self) -> RecordId {
        self.rotation
    }

    pub fn index(&self) -> u8 {
        self.index
    }

    /// How many parts the rotation has.
    pub fn parts(&self) -> u8 {
        self.parts
    }

    pub fn channel(&self) -> ChannelId {
        self.channel
    }

    pub fn prev(&self) -> Option<RecordId> {
        self.prev
    }
}

/// A complete rotation: every part, assembled. Holding one means its parts
/// are well-formed and fit together; whether its author may rotate is the
/// caller's (authority's) call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rotation {
    id: RecordId,
    server: ServerId,
    rotator: PublicKey,
    channel: ChannelId,
    epoch: u64,
    prev: Option<RecordId>,
    ephemeral: MontgomeryPoint,
    /// ver..ephemeral, exactly as encoded.
    header: Vec<u8>,
    wraps: Vec<(PublicKey, [u8; WRAPPED_KEY_LEN])>,
}

impl Rotation {
    /// Builds a rotation: a fresh epoch key wrapped for each of
    /// `recipients`, split into as few parts as fit. Returns the part bodies
    /// in order (each to be signed by `rotator` in its own `KeyRotation`
    /// record) and the new key.
    pub fn create<'a>(
        server: ServerId,
        rotator: PublicKey,
        channel: ChannelId,
        epoch: u64,
        prev: Option<RecordId>,
        recipients: impl IntoIterator<Item = &'a PublicKey>,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<(Vec<Vec<u8>>, EpochKey), Error> {
        if (epoch == 0) != prev.is_none() {
            return Err(Error::BadEpochLink);
        }
        let mut recipients: Vec<PublicKey> = recipients.into_iter().copied().collect();
        recipients.sort_unstable();
        recipients.dedup();
        if recipients.is_empty() {
            return Err(Error::NoRecipients);
        }
        if recipients.len() > MAX_RECIPIENTS {
            return Err(Error::TooManyRecipients(recipients.len()));
        }
        let points = recipients
            .iter()
            .map(|recipient| x25519_public(recipient).map(|point| (*recipient, point)))
            .collect::<Result<Vec<_>, _>>()?;

        let key = EpochKey::generate(rng);
        let mut ephemeral_secret = Zeroizing::new([0; 32]);
        rng.fill_bytes(ephemeral_secret.as_mut());
        let ephemeral = MontgomeryPoint::mul_base_clamped(*ephemeral_secret);

        let mut header = Vec::with_capacity(ROTATION_HEADER_LEN);
        header.push(VERSION);
        header.extend_from_slice(channel.as_bytes());
        header.extend_from_slice(&epoch.to_le_bytes());
        match prev {
            None => header.push(0),
            Some(prev) => {
                header.push(1);
                header.extend_from_slice(prev.as_bytes());
            }
        }
        header.extend_from_slice(ephemeral.as_bytes());
        let id = rotation_id(server, rotator, &header);
        let chunks: Vec<_> = points.chunks(WRAPS_PER_PART).collect();
        // At most MAX_PARTS (32), checked through MAX_RECIPIENTS above.
        let count = chunks.len() as u8;
        let mut parts = Vec::with_capacity(chunks.len());
        for (index, chunk) in chunks.into_iter().enumerate() {
            let mut body =
                Vec::with_capacity(PART_HEADER_LEN + ROTATION_HEADER_LEN + chunk.len() * WRAP_LEN);
            body.extend_from_slice(id.as_bytes());
            body.push(index as u8);
            body.push(count);
            body.extend_from_slice(&header);
            body.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            for (recipient, point) in chunk {
                // A valid, non-weak Ed25519 key never maps to a small-order
                // point.
                let shared = diffie_hellman(*point, &ephemeral_secret)
                    .ok_or(Error::BadMemberKey(*recipient))?;
                let ad = wrap_ad(server, rotator, &header, *recipient);
                let cipher = wrap_cipher(&shared, &ephemeral, point, &ad);
                let sealed = cipher
                    .encrypt(&XNonce::default(), Payload { msg: key.as_bytes(), aad: &ad })
                    .expect("sealing 32 bytes can't fail");
                body.extend_from_slice(recipient.as_bytes());
                body.extend_from_slice(&sealed);
            }
            parts.push(body);
        }
        Ok((parts, key))
    }

    /// Puts a rotation together from all its parts, in any order. Refuses
    /// unless there is exactly one part per index, every part names the same
    /// rotation, rotator and header, every part but the last is full, and
    /// the recipients ascend across parts: the one way [`Self::create`]
    /// splits them.
    pub fn assemble(server: ServerId, mut parts: Vec<Part>) -> Result<Rotation, Error> {
        parts.sort_by_key(|part| part.index);
        let Some(first) = parts.first() else {
            return Err(Error::MissingParts);
        };
        if parts.len() != usize::from(first.parts) {
            return Err(Error::MissingParts);
        }
        let last = parts.len() - 1;
        for (index, part) in parts.iter().enumerate() {
            if usize::from(part.index) != index
                || part.parts != first.parts
                || part.rotation != first.rotation
                || part.rotator != first.rotator
                || part.header != first.header
            {
                return Err(Error::PartMismatch);
            }
            if index < last && part.wraps.len() != WRAPS_PER_PART {
                return Err(Error::NotCanonical);
            }
        }
        let Part { rotation, rotator, channel, epoch, prev, ephemeral, ref header, .. } = *first;
        let header = header.clone();
        let mut wraps: Vec<(PublicKey, [u8; WRAPPED_KEY_LEN])> =
            Vec::with_capacity(parts.iter().map(|part| part.wraps.len()).sum());
        for part in parts {
            if let (Some((last, _)), Some((next, _))) = (wraps.last(), part.wraps.first()) {
                if last >= next {
                    return Err(Error::NotCanonical);
                }
            }
            wraps.extend(part.wraps);
        }
        Ok(Rotation {
            id: rotation,
            server,
            rotator,
            channel,
            epoch,
            prev,
            ephemeral,
            header,
            wraps,
        })
    }

    /// Decodes a rotation that fits in one part.
    pub fn decode(server: ServerId, rotator: PublicKey, body: &[u8]) -> Result<Rotation, Error> {
        Rotation::assemble(server, vec![Part::decode(server, rotator, body)?])
    }

    pub fn id(&self) -> RecordId {
        self.id
    }

    pub fn server(&self) -> ServerId {
        self.server
    }

    pub fn rotator(&self) -> PublicKey {
        self.rotator
    }

    pub fn channel(&self) -> ChannelId {
        self.channel
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn prev(&self) -> Option<RecordId> {
        self.prev
    }

    /// Who got the epoch key, ascending.
    pub fn recipients(&self) -> impl Iterator<Item = &PublicKey> + '_ {
        self.wraps.iter().map(|(recipient, _)| recipient)
    }

    pub fn is_recipient(&self, member: &PublicKey) -> bool {
        self.wraps.binary_search_by(|(recipient, _)| recipient.cmp(member)).is_ok()
    }

    /// Opens the epoch key wrapped for `member`.
    pub fn unwrap_key(&self, member: &MemberSecret) -> Result<EpochKey, Error> {
        let me = member.public();
        let index = self
            .wraps
            .binary_search_by(|(recipient, _)| recipient.cmp(&me))
            .map_err(|_| Error::NoKey)?;
        let sealed = &self.wraps[index].1;
        let point = x25519_public(&me)?;
        let shared =
            diffie_hellman(self.ephemeral, &member.secret).ok_or(Error::BadEphemeralKey)?;
        let ad = wrap_ad(self.server, self.rotator, &self.header, me);
        let cipher = wrap_cipher(&shared, &self.ephemeral, &point, &ad);
        let opened = Zeroizing::new(
            cipher
                .decrypt(&XNonce::default(), Payload { msg: sealed, aad: &ad })
                .map_err(|_| Error::UnwrapFailed)?,
        );
        let mut key = Zeroizing::new([0; KEY_LEN]);
        if opened.len() != KEY_LEN {
            return Err(Error::UnwrapFailed);
        }
        key.copy_from_slice(&opened);
        Ok(EpochKey(key))
    }

    /// The context a message sent under this rotation's key is bound to.
    pub fn message_context(&self, author: PublicKey) -> MessageContext {
        MessageContext {
            server: self.server,
            channel: self.channel,
            epoch: self.epoch,
            rotation: self.id,
            author,
        }
    }
}

fn wrap_ad(server: ServerId, rotator: PublicKey, header: &[u8], recipient: PublicKey) -> Vec<u8> {
    let mut ad = Vec::with_capacity(WRAP_AD.len() + 3 * 32 + header.len());
    ad.extend_from_slice(WRAP_AD);
    ad.extend_from_slice(server.as_bytes());
    ad.extend_from_slice(rotator.as_bytes());
    ad.extend_from_slice(header);
    ad.extend_from_slice(recipient.as_bytes());
    ad
}

/// A key used for exactly one wrap: it depends on the recipient, the
/// ephemeral key and the full header, so the all-zero nonce is never
/// repeated under one key, even if a rotator's RNG repeats an ephemeral key.
fn wrap_cipher(
    shared: &[u8; 32],
    ephemeral: &MontgomeryPoint,
    recipient: &MontgomeryPoint,
    ad: &[u8],
) -> XChaCha20Poly1305 {
    let mut material = Zeroizing::new(Vec::with_capacity(3 * 32 + ad.len()));
    material.extend_from_slice(shared);
    material.extend_from_slice(ephemeral.as_bytes());
    material.extend_from_slice(recipient.as_bytes());
    material.extend_from_slice(ad);
    let key = Zeroizing::new(blake3::derive_key(WRAP_KDF, &material));
    XChaCha20Poly1305::new(key.as_ref().into())
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.bytes.len() < n {
            return Err(Error::Truncated);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        self.array().map(u16::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, Error> {
        self.array().map(u64::from_le_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x25519_keys_match_the_ed25519_identity() {
        for seed in 0..32u8 {
            let identity = SigningKey::from_bytes(&[seed; 32]);
            let public = PublicKey::from_bytes(identity.verifying_key().to_bytes());
            let from_public = x25519_public(&public).unwrap();
            let from_secret =
                MontgomeryPoint::mul_base_clamped(*MemberSecret::new(&identity).secret);
            assert_eq!(from_public, from_secret);
        }
    }

    #[test]
    fn overhead_numbers() {
        assert_eq!(message_body_limit(), Kind::Message.max_body());
        assert_eq!(MESSAGE_OVERHEAD, 57);
        assert_eq!(WRAP_LEN, 80);
        assert_eq!(ROTATION_HEADER_LEN, 108);
        assert_eq!(PART_MAX_BODY, Kind::KeyRotation.max_body());
        assert_eq!(PART_HEADER_LEN, 34);
        assert_eq!(WRAPS_PER_PART, 203);
        assert_eq!(MAX_RECIPIENTS, 6_496);
        assert_eq!(parts_for(500), 3);
        assert_eq!(rotation_body_len(500), 40_426);
        assert_eq!(MAX_ROTATION_BYTES, 512 * 1024);
    }
}
