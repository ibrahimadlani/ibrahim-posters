//! WebP encoding.
//!
//! Quality and method are fixed rather than configurable. Both are inputs to
//! the encoded bytes but neither is in the cache key, so a request-level knob
//! would let two requests differing only in quality collide onto one entry and
//! serve each other's output.
//!
//! Changing either constant therefore requires a `RENDER_VERSION` bump once
//! anything is deployed. Note that the visual regression suite will *not*
//! catch such a change: it compares pre-encode pixels, deliberately, so that a
//! libwebp upgrade does not fail CI for a difference nobody can see. That
//! makes the encoder settings one of the few inputs to the served bytes with
//! no automated guard, which is why they are constants in one place rather
//! than arguments threaded through the pipeline.

use crate::render::source::Rgba8;
use crate::render::RenderError;

/// Delivery quality.
///
/// 82 rather than 90: the difference is not visible on the gradient-heavy
/// content posters produce, and the file is substantially smaller on a path
/// where bandwidth is paid on every edge miss.
pub const DELIVERY_QUALITY: f32 = 82.0;

/// Quality for the L1 resized-source tier.
///
/// Higher than delivery because L1 is an intermediate that will be decoded and
/// re-encoded. Compounding lossy encodes at delivery quality visibly degrades
/// the smooth gradients that dominate a blurred band.
pub const INTERMEDIATE_QUALITY: f32 = 90.0;

/// libwebp compression method.
///
/// Method 2, not the library default of 4. `PLAN.md` originally specified 4 on
/// the reasoning that it is "the knee of the quality-per-millisecond curve".
/// Measured on poster-shaped content, that is simply false — method 4 is
/// dominated by method 2 on both axes at once:
///
/// | method | time | bytes |
/// |--------|------|-------|
/// | 0 | 35.6 ms | 217 936 |
/// | 1 | 38.4 ms | 215 728 |
/// | **2** | **59.1 ms** | **210 266** |
/// | 3 | 133.4 ms | 220 242 |
/// | 4 | 133.7 ms | 221 622 |
/// | 6 | 333.0 ms | 217 440 |
///
/// Method 2 is 2.3x faster than 4 *and* produces a 5 % smaller file. Nothing
/// is traded away, so there is no judgement call to record — only a
/// measurement that contradicted an assumption.
///
/// Methods 0 and 1 are faster still but give back the size advantage, and at
/// 24 ms of saving they are not worth a larger object on every edge miss.
pub const COMPRESSION_METHOD: i32 = 2;

/// Encodes straight-alpha RGBA8 as lossy WebP.
///
/// # Arguments
///
/// * `image` — the image to encode.
/// * `quality` — 0.0 to 100.0.
///
/// # Returns
///
/// The encoded bytes.
///
/// # Errors
///
/// [`RenderError::Encode`] if the image has a zero dimension, which libwebp
/// rejects.
pub fn webp(image: &Rgba8, quality: f32) -> Result<Vec<u8>, RenderError> {
    if image.width == 0 || image.height == 0 {
        return Err(RenderError::Encode(
            "cannot encode an image with a zero dimension".to_owned(),
        ));
    }

    let mut config = webp::WebPConfig::new()
        .map_err(|()| RenderError::Encode("libwebp rejected its own defaults".to_owned()))?;
    config.quality = quality;
    config.method = COMPRESSION_METHOD;

    let encoder = webp::Encoder::from_rgba(&image.pixels, image.width, image.height);
    encoder
        .encode_advanced(&config)
        .map(|encoded| encoded.to_vec())
        .map_err(|error| RenderError::Encode(format!("{error:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmdb::probe;

    /// Builds a gradient image, which compresses like real poster content
    /// rather than like a solid colour.
    // Fixture casts: every value is bounded by the loop, so nothing is lost.
    #[allow(clippy::cast_possible_truncation)]
    fn gradient(width: u32, height: u32) -> Rgba8 {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        Rgba8::new(pixels, width, height).expect("dimensions match")
    }

    #[test]
    fn encoded_output_is_a_readable_webp() {
        let encoded = webp(&gradient(200, 300), DELIVERY_QUALITY).expect("encodes");

        // Probed with the service's own header parser: this asserts the
        // output is something the fetch path could read back, not merely that
        // some bytes were produced.
        let probed = probe::probe(&encoded).expect("output is a recognisable image");
        assert_eq!(probed.format, probe::ImageFormat::WebP);
        assert_eq!((probed.width, probed.height), (200, 300));
    }

    #[test]
    fn encoding_compresses() {
        let image = gradient(400, 600);
        let encoded = webp(&image, DELIVERY_QUALITY).expect("encodes");
        assert!(
            encoded.len() < image.pixels.len() / 4,
            "{} bytes from {} raw is not compression",
            encoded.len(),
            image.pixels.len()
        );
    }

    #[test]
    fn higher_quality_produces_a_larger_file() {
        let image = gradient(300, 450);
        let low = webp(&image, 40.0).expect("encodes");
        let high = webp(&image, 95.0).expect("encodes");
        assert!(
            high.len() > low.len(),
            "quality 95 produced {} bytes, quality 40 produced {}",
            high.len(),
            low.len()
        );
    }

    #[test]
    fn the_intermediate_tier_encodes_at_higher_quality() {
        // Asserted on the encoded result rather than on the constants, which
        // clippy correctly points out is a compile-time comparison. This
        // checks the property that actually matters: an L1 intermediate is
        // stored with more detail than the delivery output, so the second
        // lossy pass has something to work with.
        let image = gradient(240, 360);
        let delivery = webp(&image, DELIVERY_QUALITY).expect("encodes");
        let intermediate = webp(&image, INTERMEDIATE_QUALITY).expect("encodes");

        assert!(
            intermediate.len() > delivery.len(),
            "intermediate {} bytes is not larger than delivery {}",
            intermediate.len(),
            delivery.len()
        );
    }

    #[test]
    fn a_zero_dimension_is_rejected_rather_than_passed_to_libwebp() {
        let empty = Rgba8::new(Vec::new(), 0, 0).expect("valid");
        assert!(webp(&empty, DELIVERY_QUALITY).is_err());
    }

    #[test]
    fn encoding_is_deterministic() {
        // Two encodes of one image must agree, or the L2 tier would store
        // different bytes for one key depending on which process wrote first.
        let image = gradient(120, 180);
        assert_eq!(
            webp(&image, DELIVERY_QUALITY).expect("encodes"),
            webp(&image, DELIVERY_QUALITY).expect("encodes")
        );
    }

    #[test]
    fn a_single_pixel_encodes() {
        let pixel = Rgba8::new(vec![255, 0, 0, 255], 1, 1).expect("valid");
        assert!(!webp(&pixel, DELIVERY_QUALITY).expect("encodes").is_empty());
    }
}
