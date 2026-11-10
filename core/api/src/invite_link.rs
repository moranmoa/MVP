//! Invite link codec (T8-02, `docs/design/short-invite-link.md` §3, §7).
//! Pure: no network, no keys derived beyond the checksum.
//!
//! - v0: `mvs://join/<server>/<invite>/<secret>`, each 32 bytes in lower-case
//!   hex. Parsed through the open alpha; no longer created from release N+1.
//! - v1: `mvs://join/<base58(37 bytes)>` (Bitcoin alphabet, at most 51
//!   characters):
//!
//! ```text
//! offset  len  field
//! 0       1    version = 0x01
//! 1       16   invite key k
//! 17      16   server ID prefix
//! 33      4    BLAKE3 derive_key("mvs/invite-link-checksum/v1", bytes[0..33])[..4]
//! ```
//!
//! The checksum catches typos and case changes; it is not a MAC. Both forms
//! are bearer credentials: never logged, `Debug` redacted.

use std::fmt;

use records::{RecordId, ServerId};
use zeroize::Zeroizing;

pub const PREFIX: &str = "mvs://join/";
pub const V1: u8 = 0x01;
pub const KEY_LEN: usize = 16;
pub const SERVER_PREFIX_LEN: usize = 16;
const CHECKSUM_LEN: usize = 4;
const V1_LEN: usize = 1 + KEY_LEN + SERVER_PREFIX_LEN + CHECKSUM_LEN;
/// Longest v1 payload in base58 that the design allows (≤ 60 after the
/// prefix). Longer input is refused before decoding (base58 decoding is
/// quadratic).
const MAX_V1_CHARS: usize = 60;
const CHECKSUM_CONTEXT: &str = "mvs/invite-link-checksum/v1";

/// A parsed invite link.
pub enum InviteLink {
    V0 { server: ServerId, invite: RecordId, secret: Zeroizing<[u8; 32]> },
    V1 { key: Zeroizing<[u8; KEY_LEN]>, server_prefix: [u8; SERVER_PREFIX_LEN] },
}

impl fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InviteLink::V0 { .. } => f.write_str("InviteLink::V0(..)"),
            InviteLink::V1 { .. } => f.write_str("InviteLink::V1(..)"),
        }
    }
}

/// Why a link was refused; the UI words each one (`copy-exactly` for a bad
/// checksum, `update-needed` for a newer version).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// Not an `mvs://join/` link, or not either layout.
    Malformed,
    /// v1 layout with a wrong checksum: mistyped, or its case changed.
    Checksum,
    /// A version this build doesn't know.
    UnknownVersion(u8),
}

/// v1 link for invite key `key` on `server`.
pub fn encode_v1(key: &[u8; KEY_LEN], server: &ServerId) -> String {
    let mut bytes = Zeroizing::new([0u8; V1_LEN]);
    bytes[0] = V1;
    bytes[1..1 + KEY_LEN].copy_from_slice(key);
    bytes[1 + KEY_LEN..V1_LEN - CHECKSUM_LEN]
        .copy_from_slice(&server.as_bytes()[..SERVER_PREFIX_LEN]);
    let sum = checksum(&bytes[..V1_LEN - CHECKSUM_LEN]);
    bytes[V1_LEN - CHECKSUM_LEN..].copy_from_slice(&sum);
    format!("{PREFIX}{}", bs58::encode(&bytes[..]).into_string())
}

/// v0 link, as created today.
pub fn encode_v0(server: &ServerId, invite: &RecordId, secret: &[u8; 32]) -> String {
    format!(
        "{PREFIX}{}/{}/{}",
        hex::encode(server.as_bytes()),
        hex::encode(invite.as_bytes()),
        hex::encode(secret)
    )
}

/// Either layout. Surrounding whitespace is ignored; nothing else is
/// forgiven (v1 is case-sensitive, v0 hex is lower-case as created).
pub fn parse(link: &str) -> Result<InviteLink, LinkError> {
    let rest = link.trim().strip_prefix(PREFIX).ok_or(LinkError::Malformed)?;
    if rest.contains('/') {
        parse_v0(rest)
    } else {
        parse_v1(rest)
    }
}

fn parse_v0(rest: &str) -> Result<InviteLink, LinkError> {
    let mut parts = rest.split('/');
    let (server, invite, secret) = (
        parts.next().ok_or(LinkError::Malformed)?,
        parts.next().ok_or(LinkError::Malformed)?,
        parts.next().ok_or(LinkError::Malformed)?,
    );
    if parts.next().is_some() {
        return Err(LinkError::Malformed);
    }
    Ok(InviteLink::V0 {
        server: ServerId::from_bytes(hex32(server)?),
        invite: RecordId::from_bytes(hex32(invite)?),
        secret: Zeroizing::new(hex32(secret)?),
    })
}

fn hex32(part: &str) -> Result<[u8; 32], LinkError> {
    if part.len() != 64 || part.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(LinkError::Malformed);
    }
    hex::decode(part).ok().and_then(|b| b.try_into().ok()).ok_or(LinkError::Malformed)
}

fn parse_v1(rest: &str) -> Result<InviteLink, LinkError> {
    if rest.is_empty() || rest.len() > MAX_V1_CHARS {
        return Err(LinkError::Malformed);
    }
    let bytes = Zeroizing::new(bs58::decode(rest).into_vec().map_err(|_| LinkError::Malformed)?);
    // Version first: a later layout may be longer.
    match bytes.first() {
        Some(&V1) => {}
        Some(&v) if v > V1 => return Err(LinkError::UnknownVersion(v)),
        _ => return Err(LinkError::Malformed),
    }
    if bytes.len() != V1_LEN {
        return Err(LinkError::Malformed);
    }
    let (body, sum) = bytes.split_at(V1_LEN - CHECKSUM_LEN);
    if checksum(body) != sum {
        return Err(LinkError::Checksum);
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&body[1..1 + KEY_LEN]);
    let mut server_prefix = [0u8; SERVER_PREFIX_LEN];
    server_prefix.copy_from_slice(&body[1 + KEY_LEN..]);
    Ok(InviteLink::V1 { key, server_prefix })
}

fn checksum(body: &[u8]) -> [u8; CHECKSUM_LEN] {
    let full = blake3::derive_key(CHECKSUM_CONTEXT, body);
    let mut sum = [0u8; CHECKSUM_LEN];
    sum.copy_from_slice(&full[..CHECKSUM_LEN]);
    sum
}

/// Whether `server` is the one a v1 link names (its first 16 bytes).
pub fn matches_prefix(server: &ServerId, prefix: &[u8; SERVER_PREFIX_LEN]) -> bool {
    &server.as_bytes()[..SERVER_PREFIX_LEN] == prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> ServerId {
        ServerId::from_bytes(std::array::from_fn(|i| i as u8 + 0x40))
    }
    const KEY: [u8; KEY_LEN] = [0x11; KEY_LEN];

    /// Pinned: changing the layout, alphabet or checksum silently would
    /// break every link already handed out.
    const PINNED_V1: &str = "mvs://join/35G83kEaPTNk9DkdkbXtGySNKTU5PhDEotfM52FHo6VPfQS72U";
    const PINNED_CHECKSUM: &str = "33743fc5";

    #[test]
    fn the_alphabet_is_bitcoins() {
        // The standard Bitcoin base58 test vector.
        assert_eq!(bs58::encode(b"hello world").into_string(), "StV1DL6CwTryKyV");
    }

    #[test]
    fn v1_is_pinned_and_round_trips() {
        let link = encode_v1(&KEY, &server());
        assert_eq!(link, PINNED_V1);
        let mut body = vec![V1];
        body.extend_from_slice(&KEY);
        body.extend_from_slice(&server().as_bytes()[..SERVER_PREFIX_LEN]);
        assert_eq!(hex::encode(checksum(&body)), PINNED_CHECKSUM);
        // The checksum is BLAKE3 derive_key under its own context.
        assert_eq!(checksum(&body), blake3::derive_key("mvs/invite-link-checksum/v1", &body)[..4]);
        let InviteLink::V1 { key, server_prefix } = parse(&link).unwrap() else { panic!("not v1") };
        assert_eq!(*key, KEY);
        assert!(matches_prefix(&server(), &server_prefix));
        assert!(!matches_prefix(&ServerId::from_bytes([0; 32]), &server_prefix));
    }

    #[test]
    fn v1_fits_the_length_budget() {
        // Worst case: all-0xff key and prefix.
        let link = encode_v1(&[0xff; KEY_LEN], &ServerId::from_bytes([0xff; 32]));
        let payload = link.strip_prefix(PREFIX).unwrap();
        assert!(payload.len() <= 51, "{} characters", payload.len());
        // Smallest: still well-formed, no leading '1' (the version byte isn't 0).
        let small = encode_v1(&[0; KEY_LEN], &ServerId::from_bytes([0; 32]));
        assert!(!small.strip_prefix(PREFIX).unwrap().starts_with('1'));
        assert!(matches!(parse(&small), Ok(InviteLink::V1 { .. })));
    }

    #[test]
    fn v0_still_parses_and_round_trips() {
        let (invite, secret) = (RecordId::from_bytes([7; 32]), [9u8; 32]);
        let link = encode_v0(&server(), &invite, &secret);
        let InviteLink::V0 { server: s, invite: i, secret: k } =
            parse(&format!("  {link}\n")).unwrap()
        else {
            panic!("not v0")
        };
        assert_eq!((s, i, *k), (server(), invite, secret));
        // Upper-case hex, a missing part, an extra part, a short part.
        assert_eq!(
            parse(&link.to_uppercase().replace("MVS://JOIN/", PREFIX)).unwrap_err(),
            LinkError::Malformed
        );
        let parts: Vec<&str> = link.strip_prefix(PREFIX).unwrap().split('/').collect();
        for bad in [
            format!("{PREFIX}{}/{}", parts[0], parts[1]),
            format!("{link}/"),
            format!("{link}/{}", parts[2]),
            format!("{PREFIX}{}/{}/{}", parts[0], parts[1], &parts[2][..62]),
            format!("{PREFIX}{}/{}/{}zz", parts[0], parts[1], &parts[2][..62]),
        ] {
            assert_eq!(parse(&bad).unwrap_err(), LinkError::Malformed, "{bad}");
        }
    }

    #[test]
    fn hostile_v1_links_are_refused() {
        let link = encode_v1(&KEY, &server());
        let payload = link.strip_prefix(PREFIX).unwrap().to_owned();

        // One character changed: the checksum catches it (or it no longer
        // decodes to 37 bytes).
        let mut flipped = payload.clone().into_bytes();
        flipped[10] = if flipped[10] == b'2' { b'3' } else { b'2' };
        let flipped = format!("{PREFIX}{}", String::from_utf8(flipped).unwrap());
        assert!(matches!(parse(&flipped), Err(LinkError::Checksum | LinkError::Malformed)));

        // Case changed: base58 is case-sensitive.
        let swapped: String = payload
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        assert!(matches!(
            parse(&format!("{PREFIX}{swapped}")),
            Err(LinkError::Checksum | LinkError::Malformed)
        ));

        // Not base58 (0, O, I, l), a '/' inside, empty, too long, wrong prefix.
        for bad in [
            format!("{PREFIX}0{}", &payload[1..]),
            format!("{PREFIX}O{}", &payload[1..]),
            format!("{PREFIX}{}/{}", &payload[..20], &payload[20..]),
            PREFIX.to_owned(),
            format!("{PREFIX}{}", "2".repeat(52)),
            format!("{PREFIX}{}", "z".repeat(MAX_V1_CHARS + 1)),
            format!("mvs://Join/{payload}"),
            format!("https://join/{payload}"),
            payload.clone(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }

        // A valid checksum over the wrong length, and a leading zero byte.
        for len in [V1_LEN - 1, V1_LEN + 1] {
            let mut bytes = vec![V1; len];
            let sum = checksum(&bytes[..len - CHECKSUM_LEN]);
            bytes[len - CHECKSUM_LEN..].copy_from_slice(&sum);
            let link = format!("{PREFIX}{}", bs58::encode(&bytes).into_string());
            assert_eq!(parse(&link).unwrap_err(), LinkError::Malformed, "{len} bytes");
        }
        let mut zero = vec![0u8; V1_LEN];
        let sum = checksum(&zero[..V1_LEN - CHECKSUM_LEN]);
        zero[V1_LEN - CHECKSUM_LEN..].copy_from_slice(&sum);
        assert_eq!(
            parse(&format!("{PREFIX}{}", bs58::encode(&zero).into_string())).unwrap_err(),
            LinkError::Malformed
        );
    }

    #[test]
    fn a_newer_version_is_named_not_guessed() {
        let mut bytes = vec![0x02u8; V1_LEN];
        let n = bytes.len();
        let sum = checksum(&bytes[..n - CHECKSUM_LEN]);
        bytes[n - CHECKSUM_LEN..].copy_from_slice(&sum);
        let link = format!("{PREFIX}{}", bs58::encode(&bytes).into_string());
        assert_eq!(parse(&link).unwrap_err(), LinkError::UnknownVersion(2));
    }

    #[test]
    fn debug_never_shows_the_key_or_secret() {
        let shown = format!("{:?}", parse(&encode_v1(&KEY, &server())).unwrap());
        assert_eq!(shown, "InviteLink::V1(..)");
        let v0 = parse(&encode_v0(&server(), &RecordId::from_bytes([7; 32]), &[9; 32])).unwrap();
        assert_eq!(format!("{v0:?}"), "InviteLink::V0(..)");
    }
}
