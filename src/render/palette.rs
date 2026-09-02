//! Derives the badge's colours from the artwork it sits on.
//!
//! # Why a dominant colour rather than an average
//!
//! Averaging a poster yields mud: complementary regions cancel, and almost
//! every poster averages to a desaturated brown. The colour a viewer would
//! name as "the poster's colour" is the one that occupies the most area, which
//! is a mode, not a mean.
//!
//! # Why the extremes are excluded
//!
//! Nearly every poster is mostly near-black — vignettes, shadows, letterboxed
//! edges — and a plain mode returns black for all of them. Near-white behaves
//! the same way on the minority that are bright. Both are excluded so the mode
//! reflects the poster's character rather than its darkest corner.

use crate::render::source::Rgba8;
use crate::spec::Rgb;

/// Fraction of the poster height the badge colour is sampled from.
///
/// The badge sits at the top, so it takes its colour from the top. The blur
/// band occupies the bottom third and is itself a derived surface, so
/// sampling it would feed the renderer's own output back into its input.
const SAMPLE_FRACTION: f32 = 0.45;

/// Channel mean below which a pixel is treated as shadow, not colour.
const SHADOW_CUTOFF: u32 = 24;
/// Channel mean above which a pixel is treated as highlight, not colour.
const HIGHLIGHT_CUTOFF: u32 = 232;

/// Width of a histogram bin per channel. Sixteen bins per channel is coarse
/// enough that a gradient lands in one bucket rather than spreading across
/// hundreds, and fine enough to keep two distinct colours apart.
const BIN_SHIFT: u8 = 4;

/// Colours for one badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgePalette {
    /// Pill fill.
    pub fill: Rgb,
    /// Text colour.
    pub ink: Rgb,
}

/// Derives a badge palette from the artwork.
///
/// # Arguments
///
/// * `artwork` — the background, already resized to the output frame.
///
/// # Returns
///
/// The dominant colour of the poster's top region as the fill, paired with
/// whichever of black and white reads more clearly on it.
#[must_use]
pub fn from_artwork(artwork: &Rgba8) -> BadgePalette {
    let fill = dominant(artwork);
    BadgePalette {
        fill,
        ink: readable_ink(fill),
    }
}

/// Returns whichever of black and white contrasts more with `background`.
///
/// Maximising contrast rather than testing a lightness threshold is what makes
/// this correct on saturated colours: a mid-yellow and a mid-blue can share a
/// lightness while taking opposite text colours.
#[must_use]
pub fn readable_ink(background: Rgb) -> Rgb {
    if background.contrast(Rgb::WHITE) >= background.contrast(Rgb::BLACK) {
        Rgb::WHITE
    } else {
        Rgb::BLACK
    }
}

/// Returns the most-occupied colour of the artwork's top region.
///
/// Falls back to the region's plain mean when every pixel is excluded, which
/// happens on artwork that is entirely black or entirely white. The mean is a
/// poor summary in general but an exact one in precisely that case.
///
/// Casts are allowed with their bounds stated: the accumulators hold at most
/// `255 * pixel_count` with `pixel_count` bounded by the 8000 px source guard,
/// so each stays far inside `u64`, and each quotient is a channel mean and so
/// is at most 255.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn dominant(artwork: &Rgba8) -> Rgb {
    let rows = sample_rows(artwork);
    let stride = artwork.width as usize * 4;
    let mut bins = std::collections::HashMap::<[u8; 3], (u64, [u64; 3])>::new();
    let mut total = (0u64, [0u64; 3]);

    // Every second pixel in each direction. The mode of a half-resolution
    // sample and of the full image agree to within one bin, and this stage
    // runs on every cold render.
    for row in (0..rows).step_by(2) {
        let start = row * stride;
        for pixel in artwork.pixels[start..start + stride]
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(2)
        {
            let channels = [
                u64::from(pixel[0]),
                u64::from(pixel[1]),
                u64::from(pixel[2]),
            ];
            total.0 += 1;
            for (sum, value) in total.1.iter_mut().zip(channels) {
                *sum += value;
            }

            let mean = (u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3;
            if !(SHADOW_CUTOFF..=HIGHLIGHT_CUTOFF).contains(&mean) {
                continue;
            }

            let key = [
                pixel[0] >> BIN_SHIFT,
                pixel[1] >> BIN_SHIFT,
                pixel[2] >> BIN_SHIFT,
            ];
            let entry = bins.entry(key).or_insert((0, [0; 3]));
            entry.0 += 1;
            for (sum, value) in entry.1.iter_mut().zip(channels) {
                *sum += value;
            }
        }
    }

    // The winning bin's own mean, not the bin centre: quantisation is a device
    // for grouping, and reporting the centre would throw away the precision
    // the grouped pixels still carry. Ties break on the bin key so the result
    // does not depend on hash iteration order.
    let winner = bins
        .into_iter()
        .max_by_key(|(key, (count, _))| (*count, *key))
        .map(|(_, totals)| totals);

    let (count, sums) = match winner {
        Some(totals) => totals,
        None if total.0 > 0 => total,
        None => return Rgb::BLACK,
    };

    Rgb::new(
        (sums[0] / count) as u8,
        (sums[1] / count) as u8,
        (sums[2] / count) as u8,
    )
}

/// Returns how many rows of the artwork the badge colour is sampled from.
///
/// At least one row, so a one-pixel-tall image still yields a colour.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn sample_rows(artwork: &Rgba8) -> usize {
    let rows = (artwork.height as f32 * SAMPLE_FRACTION) as usize;
    rows.clamp(1, artwork.height as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an image whose top half is `top` and bottom half is `bottom`.
    fn split(top: Rgb, bottom: Rgb, width: u32, height: u32) -> Rgba8 {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let colour = if row < height / 2 { top } else { bottom };
            for _ in 0..width {
                pixels.extend_from_slice(&[colour.r, colour.g, colour.b, 255]);
            }
        }
        Rgba8::new(pixels, width, height).expect("well-formed")
    }

    fn flat(colour: Rgb) -> Rgba8 {
        split(colour, colour, 40, 60)
    }

    #[test]
    fn a_flat_image_yields_its_own_colour() {
        let colour = Rgb::new(0x5f, 0x27, 0x1a);
        assert_eq!(from_artwork(&flat(colour)).fill, colour);
    }

    #[test]
    fn the_colour_comes_from_the_top_of_the_poster() {
        // The bottom is where the blurred band goes; sampling it would feed
        // the renderer's own output back into its input.
        let top = Rgb::new(0xe8, 0xcb, 0x44);
        let bottom = Rgb::new(0x20, 0x40, 0x80);
        assert_eq!(from_artwork(&split(top, bottom, 40, 60)).fill, top);
    }

    #[test]
    fn shadow_and_highlight_do_not_win() {
        // Three quarters black, one quarter colour: the mode of the raw
        // pixels is black, but black is not what a viewer would call the
        // poster's colour.
        let colour = Rgb::new(0x9a, 0x3b, 0x22);
        let mut pixels = Vec::new();
        for row in 0..60_u32 {
            for _ in 0..40 {
                let c = if row % 4 == 0 {
                    colour
                } else {
                    Rgb::new(4, 4, 4)
                };
                pixels.extend_from_slice(&[c.r, c.g, c.b, 255]);
            }
        }
        let artwork = Rgba8::new(pixels, 40, 60).expect("well-formed");
        assert_eq!(from_artwork(&artwork).fill, colour);
    }

    #[test]
    fn an_all_black_poster_falls_back_to_its_mean() {
        // Every pixel is excluded, so the mode is undefined and the mean is
        // the honest answer.
        let artwork = flat(Rgb::new(2, 2, 2));
        assert_eq!(from_artwork(&artwork).fill, Rgb::new(2, 2, 2));
    }

    #[test]
    fn ink_maximises_contrast_against_the_fill() {
        // The five reference posters, and the text each one carries.
        let cases = [
            (Rgb::new(0x5f, 0x27, 0x1a), Rgb::WHITE),
            (Rgb::new(0x36, 0x29, 0x1b), Rgb::WHITE),
            (Rgb::new(0xe8, 0xcb, 0x44), Rgb::BLACK),
            (Rgb::new(0x5e, 0x6b, 0x51), Rgb::WHITE),
            (Rgb::new(0xa3, 0x94, 0x87), Rgb::BLACK),
        ];
        for (fill, expected) in cases {
            assert_eq!(readable_ink(fill), expected, "wrong ink for {fill}");
        }
    }

    #[test]
    fn ink_is_never_the_worse_of_the_two_choices() {
        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    let fill = Rgb::new(r, g, b);
                    let ink = readable_ink(fill);
                    let other = if ink == Rgb::WHITE {
                        Rgb::BLACK
                    } else {
                        Rgb::WHITE
                    };
                    assert!(
                        fill.contrast(ink) >= fill.contrast(other),
                        "{fill} took the lower-contrast ink"
                    );
                }
            }
        }
    }

    #[test]
    fn a_single_pixel_image_does_not_panic() {
        let artwork = Rgba8::new(vec![0x80, 0x40, 0x20, 255], 1, 1).expect("well-formed");
        assert_eq!(from_artwork(&artwork).fill, Rgb::new(0x80, 0x40, 0x20));
    }
}
