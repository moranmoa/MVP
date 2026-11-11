//! Profile picture and colour for other members (`docs/design/profile-avatar.md`,
//! #354). The picture travels in the profile post, so it is small and
//! checked strictly, on send and on every received post (untrusted). The core
//! never decodes pixels: it reads only the size, the type, the header's
//! declared dimensions and the chunk list.

use base64::Engine;

/// Bytes of image at most (design: 48×48 WebP, usually 1–2 KB).
pub const AVATAR_SMALL_MAX: usize = 2_560;
/// Declared width and height at most (Security Lead A1: no decompression bombs).
pub const AVATAR_SIDE_MAX: u32 = 64;
const WEBP_PREFIX: &str = "data:image/webp;base64,";
const PNG_PREFIX: &str = "data:image/png;base64,";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// A picture another member may see: a WebP or PNG data URL of at most
/// [`AVATAR_SMALL_MAX`] bytes, still (no animation), at most
/// [`AVATAR_SIDE_MAX`] on each side, with the right magic bytes.
pub fn valid_avatar(url: &str) -> bool {
    let (data, webp) = match (url.strip_prefix(WEBP_PREFIX), url.strip_prefix(PNG_PREFIX)) {
        (Some(data), _) => (data, true),
        (_, Some(data)) => (data, false),
        _ => return false,
    };
    // Size before decoding (Tech Lead): base64 of the cap, rounded up.
    if data.len() > AVATAR_SMALL_MAX.div_ceil(3) * 4 {
        return false;
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return false };
    if bytes.len() > AVATAR_SMALL_MAX {
        return false;
    }
    let side = if webp { webp_size(&bytes) } else { png_size(&bytes) };
    side.is_some_and(|(w, h)| (1..=AVATAR_SIDE_MAX).contains(&w) && (1..=AVATAR_SIDE_MAX).contains(&h))
}

/// `#rrggbb`.
pub fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The IHDR's width and height, if a still PNG (no `acTL`: no APNG, A2).
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(..8)? != PNG_SIGNATURE || b.get(12..16)? != b"IHDR" {
        return None;
    }
    let size = (be32(b, 16)?, be32(b, 20)?);
    // Walk the chunks: length, type, data, CRC.
    let mut at = 8;
    while at + 8 <= b.len() {
        let len = be32(b, at)? as usize;
        let kind = b.get(at + 4..at + 8)?;
        if kind == b"acTL" {
            return None;
        }
        if kind == b"IEND" {
            return Some(size);
        }
        at = at.checked_add(12)?.checked_add(len)?;
    }
    None
}

/// The canvas size of a still WebP (`VP8 `, `VP8L` or `VP8X` without the
/// animation flag, and no `ANIM`/`ANMF` chunk: A2). Every declared size must
/// be within 1..=[`AVATAR_SIDE_MAX`] and all must agree, and there must be
/// exactly one image chunk (`VP8 ` or `VP8L`): a small `VP8X` can't vouch
/// for a huge image after it (Security Lead on #359).
fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(..4)? != b"RIFF" || b.get(8..12)? != b"WEBP" {
        return None;
    }
    let mut size: Option<(u32, u32)> = None;
    let mut images = 0;
    let mut declare = |w: u32, h: u32| -> Option<()> {
        let fits = (1..=AVATAR_SIDE_MAX).contains(&w) && (1..=AVATAR_SIDE_MAX).contains(&h);
        if !fits || size.is_some_and(|s| s != (w, h)) {
            return None;
        }
        size = Some((w, h));
        Some(())
    };
    let mut at = 12;
    while at + 8 <= b.len() {
        let kind = b.get(at..at + 4)?;
        let len = le32(b, at + 4)? as usize;
        let data = b.get(at + 8..(at + 8).checked_add(len)?)?;
        match kind {
            b"ANIM" | b"ANMF" => return None,
            b"VP8X" => {
                if data.first()? & 0x02 != 0 {
                    return None; // the animation flag
                }
                let w = u32::from_le_bytes([*data.get(4)?, *data.get(5)?, *data.get(6)?, 0]) + 1;
                let h = u32::from_le_bytes([*data.get(7)?, *data.get(8)?, *data.get(9)?, 0]) + 1;
                declare(w, h)?;
            }
            b"VP8 " => {
                // Frame tag (3), start code 9d 01 2a, then 14-bit w and h.
                if data.get(3..6)? != [0x9d, 0x01, 0x2a] {
                    return None;
                }
                let w = u32::from(u16::from_le_bytes([*data.get(6)?, *data.get(7)?]) & 0x3fff);
                let h = u32::from(u16::from_le_bytes([*data.get(8)?, *data.get(9)?]) & 0x3fff);
                declare(w, h)?;
                images += 1;
            }
            b"VP8L" => {
                if *data.first()? != 0x2f {
                    return None;
                }
                let bits = le32(data, 1)?;
                declare((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)?;
                images += 1;
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = at.checked_add(8)?.checked_add(len)?.checked_add(len & 1)?;
    }
    if images != 1 {
        return None;
    }
    size
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn url(prefix: &str, bytes: &[u8]) -> String {
        format!("{prefix}{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0; 4]); // CRC: not checked
        out
    }

    /// A minimal PNG header and chunk list (no real pixels: never decoded).
    pub(crate) fn png(w: u32, h: u32, animated: bool) -> String {
        let mut b = PNG_SIGNATURE.to_vec();
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        b.extend(chunk(b"IHDR", &ihdr));
        if animated {
            b.extend(chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));
        }
        b.extend(chunk(b"IDAT", &[0; 16]));
        b.extend(chunk(b"IEND", &[]));
        url(PNG_PREFIX, &b)
    }

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> String {
        let mut body = b"WEBP".to_vec();
        for (kind, data) in chunks {
            body.extend_from_slice(*kind);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend(body);
        url(WEBP_PREFIX, &b)
    }

    pub(crate) fn webp(w: u16, h: u16) -> String {
        let mut vp8 = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
        vp8.extend_from_slice(&w.to_le_bytes());
        vp8.extend_from_slice(&h.to_le_bytes());
        vp8.extend_from_slice(&[0; 8]);
        riff(&[(b"VP8 ", vp8)])
    }

    /// Security Lead A1/A2, Tech Lead: sizes, dimensions, animation, types.
    #[test]
    fn avatars_are_checked_strictly() {
        assert!(valid_avatar(&webp(48, 48)));
        assert!(valid_avatar(&png(48, 48, false)));
        assert!(valid_avatar(&png(64, 64, false)));
        // A decompression bomb and an image one pixel too big.
        assert!(!valid_avatar(&png(10_000, 10_000, false)));
        assert!(!valid_avatar(&png(65, 65, false)));
        assert!(!valid_avatar(&webp(65, 64)));
        // Animation: APNG, an animated WebP (VP8X flag, ANIM/ANMF chunks).
        assert!(!valid_avatar(&png(48, 48, true)));
        let mut vp8x = vec![0x02, 0, 0, 0];
        vp8x.extend_from_slice(&[47, 0, 0, 47, 0, 0]);
        assert!(!valid_avatar(&riff(&[(b"VP8X", vp8x)])));
        assert!(!valid_avatar(&riff(&[(b"ANIM", vec![0; 6]), (b"ANMF", vec![0; 16])])));
        let still = vec![0, 0, 0, 0, 47, 0, 0, 47, 0, 0];
        // VP8X alone: no image chunk, refused. VP8X 48×48 with its VP8: fine.
        assert!(!valid_avatar(&riff(&[(b"VP8X", still.clone())])));
        let vp8 = |side: u16| {
            let mut d = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
            d.extend_from_slice(&side.to_le_bytes());
            d.extend_from_slice(&side.to_le_bytes());
            d.extend_from_slice(&[0; 8]);
            d
        };
        let lossless = |w: u32, h: u32| {
            let mut d = vec![0x2f];
            d.extend_from_slice(&((w - 1) | ((h - 1) << 14)).to_le_bytes());
            d
        };
        assert!(valid_avatar(&riff(&[(b"VP8X", still.clone()), (b"VP8 ", vp8(48))])));
        // Security Lead (#359): a 48×48 VP8X can't vouch for a 16,383×16,383
        // VP8L after it; sizes must agree; one image chunk only.
        assert!(!valid_avatar(&riff(&[(b"VP8X", still.clone()), (b"VP8L", lossless(16_383, 16_383))])));
        assert!(!valid_avatar(&riff(&[(b"VP8X", still.clone()), (b"VP8 ", vp8(32))])));
        assert!(!valid_avatar(&riff(&[(b"VP8 ", vp8(48)), (b"VP8 ", vp8(48))])));
        assert!(!valid_avatar(&riff(&[(b"VP8 ", vp8(48)), (b"VP8L", lossless(48, 48))])));
        // VP8L (lossless): 48×48.
        let bits: u32 = 47 | (47 << 14);
        let mut vp8l = vec![0x2f];
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert!(valid_avatar(&riff(&[(b"VP8L", vp8l)])));
        // Too large, before decoding; bad base64; the wrong magic; SVG.
        let big = format!("{WEBP_PREFIX}{}", "A".repeat(4_000));
        assert!(!valid_avatar(&big));
        assert!(!valid_avatar(&format!("{PNG_PREFIX}!!!!")));
        assert!(!valid_avatar(&webp(48, 48).replace(WEBP_PREFIX, PNG_PREFIX)));
        assert!(!valid_avatar("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="));
        // Colours.
        assert!(valid_color("#3b82f6") && valid_color("#ABCDEF"));
        assert!(!valid_color("3b82f6") && !valid_color("#3b82f") && !valid_color("#3b82fg") && !valid_color(""));
    }
}
