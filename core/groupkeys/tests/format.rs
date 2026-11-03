//! Strict decoding, and the v1 wire format pinned with a fixed RNG.

mod common;

use common::*;
use groupkeys::{
    Applied, Error, Part, Rotation, MAX_PARTS, PART_HEADER_LEN as H, WRAPS_PER_PART, WRAP_LEN,
};
use records::RecordId;

/// A valid one-part epoch-0 body for members 1 and 2: the 34-byte part
/// header, the 74-byte rotation header, the u16 count, then two wraps.
fn body() -> Vec<u8> {
    let (mut parts, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &members(&[1, 2]), &mut rng(7)).unwrap();
    assert_eq!(parts.len(), 1);
    let body = parts.remove(0);
    assert_eq!(body.len(), H + 76 + 2 * WRAP_LEN);
    body
}

fn decode(body: &[u8]) -> Result<Rotation, Error> {
    Rotation::decode(SERVER, pk(1), body)
}

#[test]
fn decode_is_strict() {
    assert!(decode(&body()).is_ok());

    // A huge count with no wraps behind it is refused before any allocation.
    let mut huge = body()[..H + 76].to_vec();
    huge[H + 74..H + 76].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode(&huge), Err(Error::TooManyRecipients(u16::MAX as usize)));
    huge[H + 74..H + 76].copy_from_slice(&(WRAPS_PER_PART as u16).to_le_bytes());
    assert_eq!(decode(&huge), Err(Error::Truncated));

    let mut zero = body()[..H + 76].to_vec();
    zero[H + 74..H + 76].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(decode(&zero), Err(Error::NoRecipients));

    let mut fewer = body();
    fewer[H + 74..H + 76].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(decode(&fewer), Err(Error::TrailingBytes));

    let mut cut = body();
    cut.pop();
    assert_eq!(decode(&cut), Err(Error::Truncated));
    assert_eq!(decode(&body()[..H - 1]), Err(Error::Truncated));

    let mut version = body();
    version[H] = 2;
    assert_eq!(decode(&version), Err(Error::UnsupportedVersion(2)));

    let mut tag = body();
    tag[H + 41] = 2;
    assert_eq!(decode(&tag), Err(Error::BadEpochLink));

    let mut epoch_without_prev = body();
    epoch_without_prev[H + 33] = 1;
    assert_eq!(decode(&epoch_without_prev), Err(Error::BadEpochLink));

    // The second wrap repeats the first recipient.
    let mut duplicate = body();
    let (first, second) = duplicate[H + 76..].split_at_mut(WRAP_LEN);
    second[..32].copy_from_slice(&first[..32]);
    assert_eq!(decode(&duplicate), Err(Error::NotCanonical));

    // Swapped wraps are no longer ascending.
    let mut swapped = body();
    let (first, second) = swapped[H + 76..].split_at_mut(WRAP_LEN);
    first.swap_with_slice(second);
    assert_eq!(decode(&swapped), Err(Error::NotCanonical));
}

#[test]
fn part_header_is_strict() {
    // Part index and count: none, past the count, more than 32.
    for (part, parts) in [(0, 0), (1, 1), (5, 2), (0, MAX_PARTS as u8 + 1)] {
        let mut bad = body();
        bad[32] = part;
        bad[33] = parts;
        assert_eq!(decode(&bad), Err(Error::BadPart), "part {part} of {parts}");
    }

    // The rotation ID must be the one the rotator and header give: a changed
    // ID, a changed header byte, another rotator or another server all fail.
    let mut id = body();
    id[0] ^= 1;
    assert_eq!(decode(&id), Err(Error::RotationIdMismatch));
    let mut channel = body();
    channel[H + 1] ^= 1;
    assert_eq!(decode(&channel), Err(Error::RotationIdMismatch));
    let mut ephemeral = body();
    ephemeral[H + 50] ^= 1;
    assert_eq!(decode(&ephemeral), Err(Error::RotationIdMismatch));
    assert_eq!(Rotation::decode(SERVER, pk(2), &body()), Err(Error::RotationIdMismatch));
    let elsewhere = records::ServerId::from_bytes([1; 32]);
    assert_eq!(Rotation::decode(elsewhere, pk(1), &body()), Err(Error::RotationIdMismatch));

    // A one-part body claiming to be part 1 of 2 decodes as a part but
    // doesn't assemble alone.
    let mut half = body();
    half[33] = 2;
    assert!(Part::decode(SERVER, pk(1), &half).is_ok());
    assert_eq!(decode(&half), Err(Error::MissingParts));
}

#[test]
fn assembly_is_strict() {
    let everyone: std::collections::BTreeSet<_> =
        (0..=WRAPS_PER_PART as u8).map(|n| pk(n.wrapping_add(10))).collect();
    let (parts, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &everyone, &mut rng(3)).unwrap();
    assert_eq!(parts.len(), 2);
    let part = |n: usize| Part::decode(SERVER, pk(1), &parts[n]).unwrap();
    assert!(Rotation::assemble(SERVER, vec![part(1), part(0)]).is_ok());
    assert_eq!(Rotation::assemble(SERVER, vec![part(0)]), Err(Error::MissingParts));
    assert_eq!(Rotation::assemble(SERVER, vec![]), Err(Error::MissingParts));
    assert_eq!(Rotation::assemble(SERVER, vec![part(0), part(0)]), Err(Error::PartMismatch));

    // A part of another rotation by the same rotator, made to claim index 1.
    let (other, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &everyone, &mut rng(4)).unwrap();
    let stranger = Part::decode(SERVER, pk(1), &other[1]).unwrap();
    assert_eq!(Rotation::assemble(SERVER, vec![part(0), stranger]), Err(Error::PartMismatch));
}

/// Pins the v1 formats: a fixed RNG must keep producing these exact bytes.
/// Changing a domain string, the KDF or the layout changes them.
#[test]
fn v1_format_is_pinned() {
    let rotation = body();
    let mut view = peer(1);
    // A fixed record ID, not a signed record: the pin covers this crate's
    // format only, not the record envelope.
    let record = RecordId::from_bytes([0x42; 32]);
    assert_eq!(view.apply(record, pk(1), &rotation), Ok((Applied::Key, vec![])));
    view.accept(record);
    let ciphertext = view.encrypt(&members(&[1, 2]), b"shalom", &mut rng(8)).unwrap();
    assert_eq!(blake3::hash(&rotation).to_hex().as_str(), GOLDEN_ROTATION);
    assert_eq!(blake3::hash(&ciphertext).to_hex().as_str(), GOLDEN_MESSAGE);
}

const GOLDEN_ROTATION: &str = "e143567f809185d7a5ebab65a330bc62d4dfbdd1f0a6ed1ea3f93b41aa26fdfc";
const GOLDEN_MESSAGE: &str = "1fff2d5cffca2af4cf9d2513aadfc33c9f1243a1a92ea15f43432db21bda35c5";
