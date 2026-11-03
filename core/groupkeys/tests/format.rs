//! Strict decoding, and the v1 wire format pinned with a fixed RNG.

mod common;

use common::*;
use groupkeys::{Applied, Error, Rotation, WRAP_LEN};
use records::RecordId;

/// A valid epoch-0 body for members 1 and 2: header is 74 bytes, then the
/// u16 count, then two wraps.
fn body() -> Vec<u8> {
    let (body, _) =
        Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &members(&[1, 2]), &mut rng(7)).unwrap();
    assert_eq!(body.len(), 76 + 2 * WRAP_LEN);
    body
}

fn decode(body: &[u8]) -> Result<Rotation, Error> {
    Rotation::decode(RecordId::from_bytes([1; 32]), SERVER, pk(1), body)
}

#[test]
fn decode_is_strict() {
    assert!(decode(&body()).is_ok());

    // A huge count with no wraps behind it is refused before any allocation.
    let mut huge = body()[..76].to_vec();
    huge[74..76].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode(&huge), Err(Error::Truncated));

    let mut zero = body()[..76].to_vec();
    zero[74..76].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(decode(&zero), Err(Error::NoRecipients));

    let mut fewer = body();
    fewer[74..76].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(decode(&fewer), Err(Error::TrailingBytes));

    let mut cut = body();
    cut.pop();
    assert_eq!(decode(&cut), Err(Error::Truncated));

    let mut version = body();
    version[0] = 2;
    assert_eq!(decode(&version), Err(Error::UnsupportedVersion(2)));

    let mut tag = body();
    tag[41] = 2;
    assert_eq!(decode(&tag), Err(Error::BadEpochLink));

    let mut epoch_without_prev = body();
    epoch_without_prev[33] = 1;
    assert_eq!(decode(&epoch_without_prev), Err(Error::BadEpochLink));

    // The second wrap repeats the first recipient.
    let mut duplicate = body();
    let (first, second) = duplicate[76..].split_at_mut(WRAP_LEN);
    second[..32].copy_from_slice(&first[..32]);
    assert_eq!(decode(&duplicate), Err(Error::NotCanonical));

    // Swapped wraps are no longer ascending.
    let mut swapped = body();
    let (first, second) = swapped[76..].split_at_mut(WRAP_LEN);
    first.swap_with_slice(second);
    assert_eq!(decode(&swapped), Err(Error::NotCanonical));
}

/// Pins the v1 formats: a fixed RNG must keep producing these exact bytes.
/// Changing a domain string, the KDF or the layout changes them.
#[test]
fn v1_format_is_pinned() {
    let rotation = body();
    let mut view = peer(1);
    // A fixed rotation ID, not a signed record: the pin covers this crate's
    // format only, not the record envelope.
    let id = RecordId::from_bytes([0x42; 32]);
    assert_eq!(view.apply(id, pk(1), &rotation), Ok(Applied::Key));
    view.accept(id);
    let ciphertext = view.encrypt(&members(&[1, 2]), b"shalom", &mut rng(8)).unwrap();
    assert_eq!(blake3::hash(&rotation).to_hex().as_str(), GOLDEN_ROTATION);
    assert_eq!(blake3::hash(&ciphertext).to_hex().as_str(), GOLDEN_MESSAGE);
}

const GOLDEN_ROTATION: &str = "412a9501496e7354dd86be6b1391a6e6381bacb04e99def9a672a414a0687eea";
const GOLDEN_MESSAGE: &str = "83176bb213b73b24e1111b2564b31b4aed18638495935c6d871f8e71900d5b8f";
