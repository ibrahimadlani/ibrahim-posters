//! Image dimension probing from file headers.
//!
//! # Why this exists
//!
//! A byte cap alone does not bound memory. A 40 KB JPEG can legitimately
//! declare dimensions that decode to several gigabytes of RGBA, so the size of
//! the compressed input says nothing useful about the size of the decoded
//! output. The guard has to read the declared dimensions and reject them
//! *before* a decoder allocates anything.
//!
//! # Why it is written by hand
//!
//! The `image` crate can read dimensions without decoding, but only for the
//! formats whose decoders are compiled in — and this crate deliberately
//! excludes `image`'s JPEG decoder in favour of `zune-jpeg`. Enabling it just
//! to read two integers would compile a second JPEG decoder into the binary
//! purely so it could be asked a question it answers from the first 20 bytes.
//!
//! Each parser below reads only the header, allocates nothing, and returns
//! `None` rather than panicking on malformed input. Every offset is bounds
//! checked, because the input is attacker-controlled by construction.

/// Image container recognised by [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// JFIF or EXIF JPEG.
    Jpeg,
    /// PNG.
    Png,
    /// WebP, lossy or lossless.
    WebP,
}

/// Dimensions declared by an image header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    /// Declared width in pixels.
    pub width: u32,
    /// Declared height in pixels.
    pub height: u32,
    /// Container the dimensions were read from.
    pub format: ImageFormat,
}

impl Dimensions {
    /// Returns the number of pixels, saturating rather than overflowing.
    ///
    /// Saturating because the product of two declared `u32` dimensions
    /// overflows a `u32` easily, and an overflow here would wrap to a small
    /// number and wave through exactly the input the guard exists to stop.
    #[must_use]
    pub const fn pixel_count(self) -> u64 {
        (self.width as u64) * (self.height as u64)
    }
}

/// Longest header prefix any parser here needs.
///
/// JPEG is the only variable case: its SOF marker can sit behind EXIF or ICC
/// segments. 64 KiB covers a full JFIF header including a large ICC profile,
/// and is the point past which a file with no SOF marker is malformed rather
/// than merely front-loaded.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Reads the dimensions declared by an image header.
///
/// # Arguments
///
/// * `bytes` — the leading bytes of the file. A complete file works; so does
///   any prefix long enough to contain the header.
///
/// # Returns
///
/// The declared dimensions, or `None` if the format is unrecognised or the
/// header is malformed or not yet complete.
///
/// # Examples
///
/// ```
/// use poster_service::tmdb::probe::{self, ImageFormat};
///
/// // A minimal PNG header: signature, IHDR length, IHDR tag, then 8x8.
/// let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// png.extend_from_slice(&13_u32.to_be_bytes());
/// png.extend_from_slice(b"IHDR");
/// png.extend_from_slice(&8_u32.to_be_bytes());
/// png.extend_from_slice(&8_u32.to_be_bytes());
///
/// let dimensions = probe::probe(&png).expect("header is complete");
/// assert_eq!((dimensions.width, dimensions.height), (8, 8));
/// assert_eq!(dimensions.format, ImageFormat::Png);
/// ```
#[must_use]
pub fn probe(bytes: &[u8]) -> Option<Dimensions> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return probe_jpeg(bytes);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return probe_png(bytes);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return probe_webp(bytes);
    }
    None
}

/// Reads big-endian `u16` at `offset`, bounds checked.
fn be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

/// Reads big-endian `u32` at `offset`, bounds checked.
fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Reads little-endian `u16` at `offset`, bounds checked.
fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Reads a 24-bit little-endian integer at `offset`, bounds checked.
fn le_u24(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 3)?;
    Some(u32::from(slice[0]) | (u32::from(slice[1]) << 8) | (u32::from(slice[2]) << 16))
}

/// Walks JPEG segments to the Start Of Frame marker.
///
/// Dimensions live in the SOF segment, whose position is not fixed: EXIF, ICC
/// and comment segments precede it in most real files, so the parser has to
/// walk the segment chain rather than read a constant offset.
fn probe_jpeg(bytes: &[u8]) -> Option<Dimensions> {
    // Past the SOI marker read by the caller.
    let mut cursor = 2_usize;

    loop {
        // Segments may be padded with any number of 0xFF fill bytes.
        while bytes.get(cursor) == Some(&0xFF) {
            cursor += 1;
        }
        let marker = *bytes.get(cursor)?;
        cursor += 1;

        match marker {
            // SOF0-SOF3, SOF5-SOF7, SOF9-SOF11, SOF13-SOF15 carry dimensions.
            // C4 (DHT), C8 (JPG) and CC (DAC) share the range but do not.
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                // Segment layout: length(2) precision(1) height(2) width(2).
                let height = be_u16(bytes, cursor + 3)?;
                let width = be_u16(bytes, cursor + 5)?;
                return Some(Dimensions {
                    width: u32::from(width),
                    height: u32::from(height),
                    format: ImageFormat::Jpeg,
                });
            }
            // Standalone markers: no length field, nothing to skip.
            0xD0..=0xD9 | 0x01 => {}
            // Start of scan: entropy-coded data follows and there is no SOF
            // ahead of it. A file that reaches here declared no dimensions.
            0xDA => return None,
            _ => {
                let length = be_u16(bytes, cursor)? as usize;
                // The length field counts itself, so anything below 2 is
                // malformed and would make the cursor move backwards.
                if length < 2 {
                    return None;
                }
                cursor += length;
            }
        }
    }
}

/// Reads the PNG IHDR chunk, which is always the first chunk.
fn probe_png(bytes: &[u8]) -> Option<Dimensions> {
    // Signature(8) length(4) type(4), then width(4) height(4).
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some(Dimensions {
        width: be_u32(bytes, 16)?,
        height: be_u32(bytes, 20)?,
        format: ImageFormat::Png,
    })
}

/// Reads a WebP canvas size from whichever chunk variant is present.
fn probe_webp(bytes: &[u8]) -> Option<Dimensions> {
    let (width, height) = match bytes.get(12..16)? {
        // Lossy: frame tag(3) start code(3) then two 14-bit dimensions.
        b"VP8 " => {
            if bytes.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            (
                u32::from(le_u16(bytes, 26)? & 0x3FFF),
                u32::from(le_u16(bytes, 28)? & 0x3FFF),
            )
        }
        // Lossless: signature byte then 14 bits of width-1 and 14 of height-1,
        // packed little-endian across four bytes.
        b"VP8L" => {
            if *bytes.get(20)? != 0x2F {
                return None;
            }
            let slice = bytes.get(21..25)?;
            let packed = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
            ((packed & 0x3FFF) + 1, ((packed >> 14) & 0x3FFF) + 1)
        }
        // Extended: flags(1) reserved(3) then two 24-bit canvas sizes, each
        // stored as the value minus one.
        b"VP8X" => (le_u24(bytes, 24)? + 1, le_u24(bytes, 27)? + 1),
        _ => return None,
    };

    Some(Dimensions {
        width,
        height,
        format: ImageFormat::WebP,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a PNG header declaring the given dimensions.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend_from_slice(&13_u32.to_be_bytes());
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out
    }

    /// Builds a JPEG header with `preceding` filler segments before the SOF.
    ///
    /// The filler models EXIF and ICC segments, which sit between SOI and SOF
    /// in most real files and are the reason the parser walks the chain.
    fn jpeg(width: u16, height: u16, preceding: usize) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        for _ in 0..preceding {
            // APP1 segment carrying 10 bytes of payload.
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&12_u16.to_be_bytes());
            out.extend_from_slice(&[0_u8; 10]);
        }
        out.extend_from_slice(&[0xFF, 0xC0]);
        out.extend_from_slice(&17_u16.to_be_bytes());
        out.push(8); // precision
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.push(3); // component count
        out.extend_from_slice(&[0_u8; 9]);
        out
    }

    /// Builds a lossy WebP header.
    fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"WEBPVP8 ");
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0]); // frame tag
        out.extend_from_slice(&[0x9D, 0x01, 0x2A]); // start code
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out
    }

    #[test]
    fn reads_png_dimensions() {
        let probed = probe(&png(1000, 1500)).expect("valid header");
        assert_eq!((probed.width, probed.height), (1000, 1500));
        assert_eq!(probed.format, ImageFormat::Png);
    }

    #[test]
    fn reads_jpeg_dimensions_behind_metadata_segments() {
        // The offset of SOF is not fixed; a parser reading a constant offset
        // would work on the first case and fail on the second.
        for preceding in [0, 1, 40] {
            let probed = probe(&jpeg(1000, 1500, preceding)).expect("valid header");
            assert_eq!(
                (probed.width, probed.height),
                (1000, 1500),
                "failed with {preceding} preceding segments"
            );
            assert_eq!(probed.format, ImageFormat::Jpeg);
        }
    }

    #[test]
    fn reads_lossy_webp_dimensions() {
        let probed = probe(&webp_lossy(1000, 1500)).expect("valid header");
        assert_eq!((probed.width, probed.height), (1000, 1500));
        assert_eq!(probed.format, ImageFormat::WebP);
    }

    #[test]
    fn reads_lossless_webp_dimensions() {
        // VP8L stores width-1 and height-1 in 14 bits each, packed LE.
        let packed: u32 = (1000_u32 - 1) | ((1500_u32 - 1) << 14);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"WEBPVP8L");
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.push(0x2F);
        out.extend_from_slice(&packed.to_le_bytes());

        let probed = probe(&out).expect("valid header");
        assert_eq!((probed.width, probed.height), (1000, 1500));
    }

    #[test]
    fn reads_extended_webp_dimensions() {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"WEBPVP8X");
        out.extend_from_slice(&10_u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]); // flags and reserved
        out.extend_from_slice(&(1000_u32 - 1).to_le_bytes()[..3]);
        out.extend_from_slice(&(1500_u32 - 1).to_le_bytes()[..3]);

        let probed = probe(&out).expect("valid header");
        assert_eq!((probed.width, probed.height), (1000, 1500));
    }

    #[test]
    fn a_truncated_header_yields_none_rather_than_panicking() {
        // Every prefix of a valid header must be handled: the fetch probes as
        // bytes arrive, so it sees each of these in turn.
        for fixture in [png(1000, 1500), jpeg(1000, 1500, 3), webp_lossy(1000, 1500)] {
            for len in 0..fixture.len() {
                let _ = probe(&fixture[..len]);
            }
        }
    }

    #[test]
    fn unrecognised_input_yields_none() {
        assert!(probe(b"").is_none());
        assert!(probe(b"not an image at all").is_none());
        assert!(probe(b"GIF89a").is_none());
        assert!(probe(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").is_none());
    }

    #[test]
    fn a_jpeg_with_no_frame_header_yields_none() {
        // SOI straight to SOS: entropy data with no dimensions declared.
        assert!(probe(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]).is_none());
    }

    #[test]
    fn a_zero_length_segment_does_not_loop_forever() {
        // A length below 2 would move the cursor backwards and spin. The
        // parser must reject rather than hang; a hang here is a denial of
        // service reachable from any upstream response.
        assert!(probe(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x00, 0x00]).is_none());
        assert!(probe(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x01, 0x00]).is_none());
    }

    #[test]
    fn pixel_count_saturates_instead_of_wrapping() {
        // Two large u32 dimensions overflow a u32 product. Wrapping would
        // produce a small number and wave through the exact input the guard
        // exists to stop.
        let huge = Dimensions {
            width: u32::MAX,
            height: u32::MAX,
            format: ImageFormat::Png,
        };
        assert!(huge.pixel_count() > u64::from(u32::MAX));
    }

    #[test]
    fn a_declared_bomb_is_visible_from_the_header_alone() {
        // The case the guard exists for: a tiny header declaring a decode
        // target of tens of gigabytes.
        let bomb = probe(&png(60_000, 60_000)).expect("valid header");
        assert_eq!(bomb.pixel_count(), 3_600_000_000);
        // At 4 bytes per pixel that is 14.4 GB of RGBA from a 24-byte header.
        assert!(bomb.pixel_count() * 4 > 14_000_000_000);
    }
}
