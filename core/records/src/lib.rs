//! Signed record envelope shared by the control log and the channels.
//!
//! Every change to a server is a [`Record`]: a small envelope signed by its
//! author with Ed25519. The envelope is encoded in one canonical binary form,
//! so the same content always produces the same bytes and the same ID.
//!
//! Wire layout (all integers little-endian):
//!
//! ```text
//! ver        u8           must be VERSION
//! server     [u8; 32]
//! scope      u8 tag       0 = control log, 1 = channel (followed by [u8; 32])
//! author     [u8; 32]     Ed25519 public key
//! seq        u64
//! deps       u16 count, then count * [u8; 32], strictly ascending
//! auth_deps  u16 count, then count * [u8; 32], strictly ascending
//! ts         u64          author-claimed ms since the Unix epoch, informational
//! kind       u16
//! body       u32 length, then the bytes (opaque here; encryption comes later)
//! sig        [u8; 64]     Ed25519 over SIGNING_CONTEXT || every byte above
//! ```
//!
//! The record ID is the BLAKE3 hash of exactly the bytes the signature covers
//! (the context plus the unsigned envelope). Leaving the signature out of the
//! ID means the ID can't be changed by re-encoding the signature.
//!
//! A [`Record`] value always holds a valid signature: the only ways to get one
//! are [`Draft::sign`] and [`Record::decode`], and its fields are read-only.

use std::fmt;

use ed25519_dalek::{Signature, Signer, VerifyingKey};

pub use ed25519_dalek::SigningKey;

/// Envelope format version this crate reads and writes.
pub const VERSION: u8 = 1;

/// Prefixed to the envelope before signing and hashing, so a record
/// signature can never be replayed as a signature over some other message.
const SIGNING_CONTEXT: &[u8] = b"mvs/record/v1\0";

/// Most heads a record may point at. A writer that sees more merges them.
pub const MAX_DEPS: usize = 64;
/// Most control-log heads a channel record may rely on.
pub const MAX_AUTH_DEPS: usize = 16;

const ID_LEN: usize = 32;
const SIG_LEN: usize = 64;

/// Upper bound on the size of any encoded record, checked before parsing.
pub const MAX_RECORD_BYTES: usize = 1 // ver
    + ID_LEN // server
    + 1 + ID_LEN // scope
    + ID_LEN // author
    + 8 // seq
    + 2 + MAX_DEPS * ID_LEN
    + 2 + MAX_AUTH_DEPS * ID_LEN
    + 8 // ts
    + 2 // kind
    + 4 + Kind::LARGEST_BODY
    + SIG_LEN;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name([u8; ID_LEN]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; ID_LEN]) -> Self {
                $name(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; ID_LEN] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "("))?;
                self.0[..4].iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
                write!(f, "..)")
            }
        }
    };
}

id_type!(
    /// Content ID of a record: BLAKE3 of its signed bytes.
    RecordId
);
id_type!(
    /// An Ed25519 public key. A user's key is their identity.
    PublicKey
);
id_type!(
    /// Identifies a server; every record of the server carries it.
    /// Derived from the owner's key and a nonce (see [`ServerId::derive`]),
    /// so only the owner can write a genesis that matches it.
    ServerId
);
id_type!(
    /// Identifies a channel within a server.
    ChannelId
);

/// A genesis body starts with this many random bytes; the rest is server
/// metadata.
pub const GENESIS_NONCE_LEN: usize = 16;

const SERVER_ID_CONTEXT: &[u8] = b"mvs/server-id/v1\0";

impl ServerId {
    /// The ID of the server whose genesis is written by `owner` with `nonce`
    /// at the start of its body. A new server picks a fresh random nonce.
    pub fn derive(owner: &PublicKey, nonce: &[u8; GENESIS_NONCE_LEN]) -> ServerId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SERVER_ID_CONTEXT);
        hasher.update(&owner.0);
        hasher.update(nonce);
        ServerId(*hasher.finalize().as_bytes())
    }
}

/// Which log a record belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// The server's control log: roles, removals, server settings.
    Control,
    Channel(ChannelId),
}

/// What a record does. Each kind has its own body size limit and its own
/// scope; unknown kinds are rejected on decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Kind {
    Genesis = 1,
    /// Body: target key, role byte.
    Grant = 2,
    /// Body: target key, then a [cut](CUT_BYTES).
    Revoke = 3,
    /// Body: target key, then a [cut](CUT_BYTES).
    Kick = 4,
    /// A text post in the control log (announcements; used by the spike).
    Post = 5,
    /// Changes the server's name, icon or description. Body: `history u8`
    /// (reserved for D30, written as 0), then the metadata.
    ServerEdit = 6,
    /// The owner closes the server for good.
    ServerClose = 7,
    /// Body: the new channel's ID, `history u8` (reserved for D30), flags,
    /// then its name.
    ChannelCreate = 8,
    /// Renames a channel or changes its flags (18+).
    ChannelEdit = 9,
    /// Body: the channel's ID.
    ChannelDelete = 10,
    /// A signed invite with an expiry and a use limit. Its record ID is what
    /// an invite link carries.
    Invite = 11,
    /// Withdraws an invite, naming its record ID.
    RevokeInvite = 12,
    /// The author joins the server through an invite, which it names and
    /// depends on.
    Join = 13,
    /// The author leaves the server. Body: a [cut](CUT_BYTES).
    Leave = 14,
    /// The author's age claim (B7).
    AgeClaim = 15,
    /// A chat message in a channel.
    Message = 16,
    /// Replaces the text of the author's own message. Body: the message's
    /// record ID, then the new text.
    MessageEdit = 17,
    /// A tombstone for a message. Body: the message's record ID.
    MessageDelete = 18,
    /// One part of a channel key rotation. Body: a
    /// [header](ROTATION_HEADER_LEN), then what `groupkeys` puts there.
    KeyRotation = 19,
    /// The owner lets a kicked key join again. Body: the key.
    Unban = 20,
}

/// Most channels a cut names.
pub const MAX_CUT_ENTRIES: usize = 64;

/// Largest cut: the channel heads a kick, revoke or leave was signed on,
/// which later decide which of the target's channel records are voided. A
/// cut is `count u8`, then `count` pairs of (channel ID, head record ID),
/// strictly ascending by channel ID. Records checks only the size; the
/// authority parses it.
pub const CUT_BYTES: usize = 1 + MAX_CUT_ENTRIES * 2 * ID_LEN;

/// Most parts one key rotation is split into (R7): about 200 members fit in
/// a part, so 8 parts cover about 1,600 members.
pub const MAX_ROTATION_PARTS: u8 = 8;

/// A key rotation body starts with this header: `rotation [u8; 32]`, the
/// same in every part of one rotation (`groupkeys` decides how it is
/// derived), then `part u8` and `parts u8`, with `part < parts` and
/// `1 <= parts <=` [`MAX_ROTATION_PARTS`]. The rest is opaque here.
pub const ROTATION_HEADER_LEN: usize = ID_LEN + 2;

impl Kind {
    const ALL: [Kind; 20] = [
        Kind::Genesis,
        Kind::Grant,
        Kind::Revoke,
        Kind::Kick,
        Kind::Post,
        Kind::ServerEdit,
        Kind::ServerClose,
        Kind::ChannelCreate,
        Kind::ChannelEdit,
        Kind::ChannelDelete,
        Kind::Invite,
        Kind::RevokeInvite,
        Kind::Join,
        Kind::Leave,
        Kind::AgeClaim,
        Kind::Message,
        Kind::MessageEdit,
        Kind::MessageDelete,
        Kind::KeyRotation,
        Kind::Unban,
    ];

    const LARGEST_BODY: usize = 16 * 1024;

    pub fn code(self) -> u16 {
        self as u16
    }

    pub fn from_code(code: u16) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// Largest body accepted for this kind, in bytes. Leaves room for the
    /// encryption overhead that will come later.
    pub fn max_body(self) -> usize {
        match self {
            // Server name, icon reference, description.
            Kind::Genesis | Kind::ServerEdit => 2 * 1024,
            // Target key plus a role byte, with room for future fields.
            Kind::Grant => 128,
            Kind::Revoke | Kind::Kick => ID_LEN + CUT_BYTES,
            Kind::Leave => CUT_BYTES,
            // Expiry and use limit, or an invite's record ID, with room for
            // future fields.
            Kind::Invite | Kind::RevokeInvite | Kind::Join => 128,
            // A channel ID or a key, with room for future fields.
            Kind::ServerClose | Kind::ChannelDelete | Kind::AgeClaim | Kind::Unban => 64,
            Kind::MessageDelete => 64,
            // Channel ID, history byte, flags and a name.
            Kind::ChannelCreate | Kind::ChannelEdit => 512,
            Kind::Post => 4 * 1024,
            Kind::Message | Kind::MessageEdit | Kind::KeyRotation => Kind::LARGEST_BODY,
        }
    }

    fn allowed_in(self, scope: Scope) -> bool {
        match self {
            Kind::Message | Kind::MessageEdit | Kind::MessageDelete | Kind::KeyRotation => {
                matches!(scope, Scope::Channel(_))
            }
            _ => scope == Scope::Control,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    TooLarge {
        len: usize,
    },
    Truncated,
    TrailingBytes,
    UnsupportedVersion(u8),
    UnknownScope(u8),
    UnknownKind(u16),
    KindNotAllowedInScope(Kind),
    BodyTooLarge {
        kind: Kind,
        len: usize,
        max: usize,
    },
    TooManyDeps(usize),
    TooManyAuthDeps(usize),
    /// deps or auth_deps not strictly ascending (unsorted or duplicated).
    NotCanonical,
    /// Control-log records rely on their own deps; auth_deps must be empty.
    AuthDepsInControlScope,
    /// A genesis must have no deps.
    GenesisWithDeps,
    /// The genesis author and nonce don't hash to the record's server ID.
    GenesisServerMismatch,
    /// A key rotation body without a valid part header (see
    /// [`ROTATION_HEADER_LEN`]).
    BadRotationHeader,
    BadAuthorKey,
    BadSignature,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::TooLarge { len } => {
                write!(f, "record is {len} bytes, limit is {MAX_RECORD_BYTES}")
            }
            RecordError::Truncated => write!(f, "record is truncated"),
            RecordError::TrailingBytes => write!(f, "bytes after the signature"),
            RecordError::UnsupportedVersion(ver) => write!(f, "unsupported version {ver}"),
            RecordError::UnknownScope(tag) => write!(f, "unknown scope tag {tag}"),
            RecordError::UnknownKind(code) => write!(f, "unknown record kind {code}"),
            RecordError::KindNotAllowedInScope(kind) => {
                write!(f, "{kind:?} records are not allowed in this scope")
            }
            RecordError::BodyTooLarge { kind, len, max } => {
                write!(f, "{kind:?} body is {len} bytes, limit is {max}")
            }
            RecordError::TooManyDeps(n) => write!(f, "{n} deps, limit is {MAX_DEPS}"),
            RecordError::TooManyAuthDeps(n) => {
                write!(f, "{n} auth_deps, limit is {MAX_AUTH_DEPS}")
            }
            RecordError::NotCanonical => write!(f, "deps are not strictly ascending"),
            RecordError::AuthDepsInControlScope => {
                write!(f, "control-log records can't carry auth_deps")
            }
            RecordError::BadAuthorKey => write!(f, "author is not a valid Ed25519 key"),
            RecordError::GenesisWithDeps => write!(f, "a genesis can't have deps"),
            RecordError::GenesisServerMismatch => {
                write!(f, "genesis author and nonce don't match the server ID")
            }
            RecordError::BadRotationHeader => write!(f, "key rotation part header is invalid"),
            RecordError::BadSignature => write!(f, "signature does not verify"),
        }
    }
}

impl std::error::Error for RecordError {}

/// The fields a record is made of, before the author signs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub server: ServerId,
    pub scope: Scope,
    pub seq: u64,
    /// Head IDs the author had seen. Sorted and deduplicated on signing.
    pub deps: Vec<RecordId>,
    /// Control-log heads a channel record relies on for permission.
    pub auth_deps: Vec<RecordId>,
    pub ts: u64,
    pub kind: Kind,
    pub body: Vec<u8>,
}

impl Draft {
    /// Validates the draft against the format limits and signs it.
    pub fn sign(mut self, key: &SigningKey) -> Result<Record, RecordError> {
        self.deps.sort_unstable();
        self.deps.dedup();
        self.auth_deps.sort_unstable();
        self.auth_deps.dedup();
        let fields = Fields {
            server: self.server,
            scope: self.scope,
            author: PublicKey(key.verifying_key().to_bytes()),
            seq: self.seq,
            deps: self.deps,
            auth_deps: self.auth_deps,
            ts: self.ts,
            kind: self.kind,
            body: self.body,
        };
        fields.validate()?;
        let message = fields.signing_message();
        let sig = key.sign(&message).to_bytes();
        let id = RecordId(*blake3::hash(&message).as_bytes());
        Ok(Record { fields, sig, id })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fields {
    server: ServerId,
    scope: Scope,
    author: PublicKey,
    seq: u64,
    deps: Vec<RecordId>,
    auth_deps: Vec<RecordId>,
    ts: u64,
    kind: Kind,
    body: Vec<u8>,
}

impl Fields {
    /// Format rules that don't depend on how the bytes were laid out.
    fn validate(&self) -> Result<(), RecordError> {
        if !self.kind.allowed_in(self.scope) {
            return Err(RecordError::KindNotAllowedInScope(self.kind));
        }
        if self.scope == Scope::Control && !self.auth_deps.is_empty() {
            return Err(RecordError::AuthDepsInControlScope);
        }
        if self.kind == Kind::Genesis {
            if !self.deps.is_empty() {
                return Err(RecordError::GenesisWithDeps);
            }
            let nonce = self.body.first_chunk::<GENESIS_NONCE_LEN>();
            if nonce.is_none_or(|nonce| ServerId::derive(&self.author, nonce) != self.server) {
                return Err(RecordError::GenesisServerMismatch);
            }
        }
        if self.deps.len() > MAX_DEPS {
            return Err(RecordError::TooManyDeps(self.deps.len()));
        }
        if self.auth_deps.len() > MAX_AUTH_DEPS {
            return Err(RecordError::TooManyAuthDeps(self.auth_deps.len()));
        }
        if !strictly_ascending(&self.deps) || !strictly_ascending(&self.auth_deps) {
            return Err(RecordError::NotCanonical);
        }
        let max = self.kind.max_body();
        if self.body.len() > max {
            return Err(RecordError::BodyTooLarge { kind: self.kind, len: self.body.len(), max });
        }
        if self.kind == Kind::KeyRotation {
            let header = self.body.get(ID_LEN..ROTATION_HEADER_LEN);
            let valid =
                header.is_some_and(|h| (1..=MAX_ROTATION_PARTS).contains(&h[1]) && h[0] < h[1]);
            if !valid {
                return Err(RecordError::BadRotationHeader);
            }
        }
        Ok(())
    }

    /// SIGNING_CONTEXT followed by the unsigned envelope.
    fn signing_message(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            SIGNING_CONTEXT.len()
                + 128
                + (self.deps.len() + self.auth_deps.len()) * ID_LEN
                + self.body.len()
                + SIG_LEN,
        );
        out.extend_from_slice(SIGNING_CONTEXT);
        out.push(VERSION);
        out.extend_from_slice(&self.server.0);
        match self.scope {
            Scope::Control => out.push(0),
            Scope::Channel(channel) => {
                out.push(1);
                out.extend_from_slice(&channel.0);
            }
        }
        out.extend_from_slice(&self.author.0);
        out.extend_from_slice(&self.seq.to_le_bytes());
        for list in [&self.deps, &self.auth_deps] {
            out.extend_from_slice(&(list.len() as u16).to_le_bytes());
            for id in list {
                out.extend_from_slice(&id.0);
            }
        }
        out.extend_from_slice(&self.ts.to_le_bytes());
        out.extend_from_slice(&self.kind.code().to_le_bytes());
        out.extend_from_slice(&(self.body.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

fn strictly_ascending(ids: &[RecordId]) -> bool {
    ids.windows(2).all(|pair| pair[0] < pair[1])
}

/// A signed record. Holding one means its signature has been verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    fields: Fields,
    sig: [u8; SIG_LEN],
    id: RecordId,
}

impl Record {
    pub fn id(&self) -> RecordId {
        self.id
    }

    pub fn ver(&self) -> u8 {
        VERSION
    }

    pub fn server(&self) -> ServerId {
        self.fields.server
    }

    pub fn scope(&self) -> Scope {
        self.fields.scope
    }

    pub fn author(&self) -> PublicKey {
        self.fields.author
    }

    pub fn seq(&self) -> u64 {
        self.fields.seq
    }

    pub fn deps(&self) -> &[RecordId] {
        &self.fields.deps
    }

    pub fn auth_deps(&self) -> &[RecordId] {
        &self.fields.auth_deps
    }

    /// Author-claimed time. Informational: never used for ordering.
    pub fn ts(&self) -> u64 {
        self.fields.ts
    }

    pub fn kind(&self) -> Kind {
        self.fields.kind
    }

    pub fn body(&self) -> &[u8] {
        &self.fields.body
    }

    pub fn signature(&self) -> &[u8; SIG_LEN] {
        &self.sig
    }

    /// The canonical wire form.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.fields.signing_message();
        out.drain(..SIGNING_CONTEXT.len());
        out.extend_from_slice(&self.sig);
        out
    }

    /// Parses and verifies a record. Anything that isn't the canonical
    /// encoding of a validly signed record within the limits is rejected.
    pub fn decode(bytes: &[u8]) -> Result<Record, RecordError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(RecordError::TooLarge { len: bytes.len() });
        }
        let mut reader = Reader { bytes };
        let ver = reader.u8()?;
        if ver != VERSION {
            return Err(RecordError::UnsupportedVersion(ver));
        }
        let server = ServerId(reader.id()?);
        let scope = match reader.u8()? {
            0 => Scope::Control,
            1 => Scope::Channel(ChannelId(reader.id()?)),
            tag => return Err(RecordError::UnknownScope(tag)),
        };
        let author = PublicKey(reader.id()?);
        let seq = reader.u64()?;
        let deps = reader.ids(MAX_DEPS, RecordError::TooManyDeps)?;
        let auth_deps = reader.ids(MAX_AUTH_DEPS, RecordError::TooManyAuthDeps)?;
        let ts = reader.u64()?;
        let code = reader.u16()?;
        let kind = Kind::from_code(code).ok_or(RecordError::UnknownKind(code))?;
        let body_len = reader.u32()? as usize;
        let max = kind.max_body();
        if body_len > max {
            return Err(RecordError::BodyTooLarge { kind, len: body_len, max });
        }
        let body = reader.take(body_len)?.to_vec();
        let sig: [u8; SIG_LEN] = reader.take(SIG_LEN)?.try_into().expect("length checked");
        if !reader.bytes.is_empty() {
            return Err(RecordError::TrailingBytes);
        }

        let fields = Fields { server, scope, author, seq, deps, auth_deps, ts, kind, body };
        fields.validate()?;
        let message = fields.signing_message();
        let key = VerifyingKey::from_bytes(&author.0).map_err(|_| RecordError::BadAuthorKey)?;
        key.verify_strict(&message, &Signature::from_bytes(&sig))
            .map_err(|_| RecordError::BadSignature)?;
        let id = RecordId(*blake3::hash(&message).as_bytes());
        Ok(Record { fields, sig, id })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], RecordError> {
        if self.bytes.len() < n {
            return Err(RecordError::Truncated);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], RecordError> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8, RecordError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, RecordError> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, RecordError> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, RecordError> {
        self.array().map(u64::from_le_bytes)
    }

    fn id(&mut self) -> Result<[u8; ID_LEN], RecordError> {
        self.array()
    }

    fn ids(
        &mut self,
        max: usize,
        too_many: fn(usize) -> RecordError,
    ) -> Result<Vec<RecordId>, RecordError> {
        let count = self.u16()? as usize;
        if count > max {
            return Err(too_many(count));
        }
        (0..count).map(|_| self.id().map(RecordId)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn draft() -> Draft {
        Draft {
            server: ServerId::from_bytes([1; 32]),
            scope: Scope::Control,
            seq: 7,
            deps: vec![RecordId::from_bytes([9; 32]), RecordId::from_bytes([3; 32])],
            auth_deps: vec![],
            ts: 1_700_000_000_000,
            kind: Kind::Post,
            body: b"shalom".to_vec(),
        }
    }

    /// Byte offset of the body inside the wire form of `draft()`.
    fn body_offset(record: &Record) -> usize {
        record.encode().len() - SIG_LEN - record.body().len()
    }

    #[test]
    fn round_trip() {
        let record = draft().sign(&key(1)).unwrap();
        let decoded = Record::decode(&record.encode()).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.body(), b"shalom");
        assert_eq!(decoded.author(), PublicKey(key(1).verifying_key().to_bytes()));
        assert_eq!(decoded.encode(), record.encode());
    }

    #[test]
    fn channel_record_round_trip() {
        let record = Draft {
            scope: Scope::Channel(ChannelId::from_bytes([4; 32])),
            auth_deps: vec![RecordId::from_bytes([5; 32])],
            kind: Kind::Message,
            ..draft()
        }
        .sign(&key(2))
        .unwrap();
        assert_eq!(Record::decode(&record.encode()).unwrap(), record);
    }

    #[test]
    fn signing_sorts_and_dedups_deps() {
        let mut messy = draft();
        messy.deps.push(RecordId::from_bytes([3; 32]));
        let record = messy.sign(&key(1)).unwrap();
        assert_eq!(record.deps(), &[RecordId([3; 32]), RecordId([9; 32])]);
        assert_eq!(record.id(), draft().sign(&key(1)).unwrap().id());
    }

    #[test]
    fn id_is_stable() {
        let a = draft().sign(&key(1)).unwrap();
        let b = draft().sign(&key(1)).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(Record::decode(&a.encode()).unwrap().id(), a.id());
        // Pins the format: changing the encoding changes every ID.
        assert_eq!(a.id().to_string(), GOLDEN_ID);

        let other = Draft { seq: 8, ..draft() }.sign(&key(1)).unwrap();
        assert_ne!(other.id(), a.id());
        let other_author = draft().sign(&key(2)).unwrap();
        assert_ne!(other_author.id(), a.id());
    }

    const GOLDEN_ID: &str = "645946b1558de504af00d486f14bdfb4c0e47674f00964bd5a56bccb97a3d7aa";

    #[test]
    fn tampered_body_is_rejected() {
        let record = draft().sign(&key(1)).unwrap();
        let mut bytes = record.encode();
        let at = body_offset(&record);
        bytes[at] ^= 1;
        assert_eq!(Record::decode(&bytes), Err(RecordError::BadSignature));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let record = draft().sign(&key(1)).unwrap();
        let mut bytes = record.encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(Record::decode(&bytes), Err(RecordError::BadSignature));
    }

    #[test]
    fn swapped_author_is_rejected() {
        let record = draft().sign(&key(1)).unwrap();
        let mut bytes = record.encode();
        let author_at = 1 + ID_LEN + 1;
        bytes[author_at..author_at + ID_LEN].copy_from_slice(&key(2).verifying_key().to_bytes());
        assert_eq!(Record::decode(&bytes), Err(RecordError::BadSignature));
    }

    #[test]
    fn oversized_body_is_rejected_on_sign_and_decode() {
        let max = Kind::Grant.max_body();
        let too_big = Draft { kind: Kind::Grant, body: vec![0; max + 1], ..draft() };
        assert_eq!(
            too_big.clone().sign(&key(1)),
            Err(RecordError::BodyTooLarge { kind: Kind::Grant, len: max + 1, max })
        );

        // A peer that skips the check still can't get it past decode.
        let fields = Fields {
            server: too_big.server,
            scope: too_big.scope,
            author: PublicKey(key(1).verifying_key().to_bytes()),
            seq: too_big.seq,
            deps: vec![],
            auth_deps: vec![],
            ts: too_big.ts,
            kind: too_big.kind,
            body: too_big.body,
        };
        let mut bytes = fields.signing_message();
        bytes.extend_from_slice(&key(1).sign(&bytes).to_bytes());
        bytes.drain(..SIGNING_CONTEXT.len());
        assert!(matches!(Record::decode(&bytes), Err(RecordError::BodyTooLarge { .. })));
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let bytes = vec![VERSION; MAX_RECORD_BYTES + 1];
        assert_eq!(
            Record::decode(&bytes),
            Err(RecordError::TooLarge { len: MAX_RECORD_BYTES + 1 })
        );
    }

    #[test]
    fn non_canonical_encodings_are_rejected() {
        let record = draft().sign(&key(1)).unwrap();
        let bytes = record.encode();

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(Record::decode(&trailing), Err(RecordError::TrailingBytes));

        assert_eq!(Record::decode(&bytes[..bytes.len() - 1]), Err(RecordError::Truncated));

        // Swap the two deps so they're no longer ascending.
        let deps_at = 1 + ID_LEN + 1 + ID_LEN + 8 + 2;
        let mut unsorted = bytes.clone();
        let (first, second) = unsorted[deps_at..deps_at + 2 * ID_LEN].split_at_mut(ID_LEN);
        first.swap_with_slice(second);
        assert_eq!(Record::decode(&unsorted), Err(RecordError::NotCanonical));

        let mut version = bytes.clone();
        version[0] = 2;
        assert_eq!(Record::decode(&version), Err(RecordError::UnsupportedVersion(2)));
    }

    #[test]
    fn kinds_stay_in_their_scope() {
        let message_in_control = Draft { kind: Kind::Message, ..draft() };
        assert_eq!(
            message_in_control.sign(&key(1)),
            Err(RecordError::KindNotAllowedInScope(Kind::Message))
        );
        let auth_deps_in_control =
            Draft { auth_deps: vec![RecordId::from_bytes([5; 32])], ..draft() };
        assert_eq!(auth_deps_in_control.sign(&key(1)), Err(RecordError::AuthDepsInControlScope));
    }

    /// A body of `len` bytes that passes the kind's own header check.
    fn body_for(kind: Kind, len: usize) -> Vec<u8> {
        let mut body = vec![0; len];
        if kind == Kind::KeyRotation {
            body[ID_LEN + 1] = 1; // part 0 of 1
        }
        body
    }

    #[test]
    fn every_kind_has_one_scope_and_a_body_limit() {
        let channel = Scope::Channel(ChannelId::from_bytes([4; 32]));
        let codes: Vec<u16> = Kind::ALL.iter().map(|kind| kind.code()).collect();
        assert_eq!(codes, (1..=20).collect::<Vec<u16>>(), "codes are dense and in order");
        // The genesis has its own tests: its body must carry the nonce.
        for kind in Kind::ALL.into_iter().filter(|kind| *kind != Kind::Genesis) {
            assert_eq!(Kind::from_code(kind.code()), Some(kind));
            let in_channel = matches!(
                kind,
                Kind::Message | Kind::MessageEdit | Kind::MessageDelete | Kind::KeyRotation
            );
            let (home, other) =
                if in_channel { (channel, Scope::Control) } else { (Scope::Control, channel) };
            let auth_deps = if in_channel { vec![RecordId::from_bytes([5; 32])] } else { vec![] };
            let at = |len| Draft {
                scope: home,
                kind,
                body: body_for(kind, len),
                auth_deps: auth_deps.clone(),
                ..draft()
            };
            let max = kind.max_body();
            let at_limit = at(max).sign(&key(1)).unwrap();
            assert_eq!(Record::decode(&at_limit.encode()).unwrap(), at_limit);
            assert_eq!(
                at(max + 1).sign(&key(1)),
                Err(RecordError::BodyTooLarge { kind, len: max + 1, max })
            );
            let elsewhere = Draft { scope: other, kind, auth_deps: vec![], ..at(max) };
            assert_eq!(elsewhere.sign(&key(1)), Err(RecordError::KindNotAllowedInScope(kind)));
        }
    }

    #[test]
    fn removals_have_room_for_a_full_cut() {
        assert_eq!(CUT_BYTES, 4097);
        assert_eq!(Kind::Kick.max_body(), 4129);
        assert_eq!(Kind::Revoke.max_body(), 4129);
        assert!(Kind::Leave.max_body() >= CUT_BYTES);
    }

    #[test]
    fn key_rotation_parts_carry_a_valid_header() {
        let channel = Scope::Channel(ChannelId::from_bytes([4; 32]));
        let rotation = |header: &[u8]| {
            let mut body = vec![7; ID_LEN];
            body.extend_from_slice(header);
            body.extend_from_slice(b"wraps");
            Draft { scope: channel, kind: Kind::KeyRotation, body, ..draft() }.sign(&key(1))
        };
        for (part, parts) in [(0, 1), (0, 8), (7, 8), (2, 3)] {
            let record = rotation(&[part, parts]).unwrap();
            assert_eq!(Record::decode(&record.encode()).unwrap(), record);
        }
        for bad in [&[0, 0][..], &[1, 1], &[8, 8], &[0, 9], &[3, 2], &[0], &[]] {
            assert_eq!(rotation(bad), Err(RecordError::BadRotationHeader), "header {bad:?}");
        }
        // The check runs on decode too: patch a valid part into part 1 of 1.
        let record = rotation(&[0, 1]).unwrap();
        let mut bytes = record.encode();
        let at = body_offset(&record) + ID_LEN;
        bytes[at] = 1;
        assert_eq!(Record::decode(&bytes), Err(RecordError::BadRotationHeader));
    }

    #[test]
    fn every_record_fits_the_global_limit() {
        let record = Draft {
            scope: Scope::Channel(ChannelId::from_bytes([4; 32])),
            deps: (0..MAX_DEPS as u8).map(|n| RecordId::from_bytes([n; 32])).collect(),
            auth_deps: (0..MAX_AUTH_DEPS as u8).map(|n| RecordId::from_bytes([n; 32])).collect(),
            kind: Kind::Message,
            body: vec![0; Kind::Message.max_body()],
            ..draft()
        }
        .sign(&key(1))
        .unwrap();
        assert_eq!(record.encode().len(), MAX_RECORD_BYTES);
        assert_eq!(Record::decode(&record.encode()).unwrap(), record);
    }

    fn genesis_draft(owner: &SigningKey, nonce: [u8; GENESIS_NONCE_LEN]) -> Draft {
        let owner_key = PublicKey(owner.verifying_key().to_bytes());
        Draft {
            server: ServerId::derive(&owner_key, &nonce),
            deps: vec![],
            kind: Kind::Genesis,
            body: [&nonce[..], b"server name"].concat(),
            ..draft()
        }
    }

    #[test]
    fn genesis_by_the_owner_round_trips() {
        let record = genesis_draft(&key(1), [7; GENESIS_NONCE_LEN]).sign(&key(1)).unwrap();
        assert_eq!(Record::decode(&record.encode()).unwrap(), record);
    }

    #[test]
    fn nobody_else_can_write_a_genesis_for_the_server() {
        // Same server ID and nonce, signed by an attacker's key.
        let forged = genesis_draft(&key(1), [7; GENESIS_NONCE_LEN]);
        assert_eq!(forged.sign(&key(2)), Err(RecordError::GenesisServerMismatch));

        // The attacker's own genesis with a different nonce names another server.
        let mut other_nonce = genesis_draft(&key(1), [7; GENESIS_NONCE_LEN]);
        other_nonce.body[0] ^= 1;
        assert_eq!(other_nonce.sign(&key(1)), Err(RecordError::GenesisServerMismatch));
    }

    #[test]
    fn forged_genesis_bytes_are_rejected_on_decode() {
        // A genuine genesis for the attacker's own server, then the server
        // field rewritten to the victim's: decode must refuse it before the
        // signature is even checked.
        let victim = genesis_draft(&key(1), [7; GENESIS_NONCE_LEN]).sign(&key(1)).unwrap();
        let mine = genesis_draft(&key(2), [7; GENESIS_NONCE_LEN]).sign(&key(2)).unwrap();
        let mut bytes = mine.encode();
        bytes[1..1 + ID_LEN].copy_from_slice(victim.server().as_bytes());
        assert_eq!(Record::decode(&bytes), Err(RecordError::GenesisServerMismatch));
    }

    #[test]
    fn genesis_needs_a_nonce_and_no_deps() {
        let short =
            Draft { body: vec![0; GENESIS_NONCE_LEN - 1], ..genesis_draft(&key(1), [0; 16]) };
        assert_eq!(short.sign(&key(1)), Err(RecordError::GenesisServerMismatch));
        let with_deps =
            Draft { deps: vec![RecordId::from_bytes([1; 32])], ..genesis_draft(&key(1), [0; 16]) };
        assert_eq!(with_deps.sign(&key(1)), Err(RecordError::GenesisWithDeps));
    }
}
