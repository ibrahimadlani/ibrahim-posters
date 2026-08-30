//! Every numeric limit in the system, in one place.
//!
//! Centralised because a limit that appears in two files eventually disagrees
//! with itself, and a disagreement here is not a visual bug — it splits one
//! cache entry into two and costs a render on every request that would
//! otherwise have hit.
//!
//! # Invariant
//!
//! Clamping runs *after* preset merge, never before. Clamping an override and
//! then merging would let two requests that clamp to the same value survive as
//! distinct specifications, which is exactly the cache fragmentation the
//! content-addressed key exists to prevent.

use std::ops::RangeInclusive;

/// Height of the blurred band as a fraction of poster height.
///
/// Floored at 0.05 because a band thinner than that is invisible at w1000, and
/// capped at 0.60 because beyond it the blur reads as a defect rather than a
/// treatment.
pub const BLUR_BAND_FRACTION: RangeInclusive<f32> = 0.05..=0.60;

/// Gaussian sigma at w1000, in pixels.
///
/// The upper bound is not aesthetic. The blur runs at a downscale factor of 8
/// (see `render::blur`), so a sigma of 96 becomes a radius of 12 at working
/// resolution; past roughly that point the three-box approximation starts to
/// show banding on smooth gradients.
pub const BLUR_SIGMA: RangeInclusive<f32> = 0.0..=96.0;

/// Peak opacity of the darkening ramp at the bottom edge.
///
/// Capped below 1.0 so the background is never fully crushed to black: the
/// darkening exists to make the logo legible, and a fully opaque ramp would
/// discard the artwork it is supposed to sit on.
pub const DARKEN_STRENGTH: RangeInclusive<f32> = 0.0..=0.95;

/// Logo width as a fraction of poster width.
pub const LOGO_WIDTH_FRACTION: RangeInclusive<f32> = 0.10..=0.90;

/// Distance from the bottom edge to the logo, as a fraction of poster height.
pub const LOGO_BOTTOM_FRACTION: RangeInclusive<f32> = 0.02..=0.40;

/// Badge row height in pixels at w1000.
pub const BADGE_HEIGHT: RangeInclusive<u32> = 24..=96;

/// Longest badge text, in grapheme clusters after NFC normalisation.
pub const BADGE_TEXT_GRAPHEMES: usize = 32;

/// Most badges that may appear in the top row.
///
/// Six 32-grapheme badges cannot fit across a 1000 px poster, so this is a
/// cheap early rejection rather than the real constraint; the real one is
/// measured width, checked in `render::badges` once the font is known.
pub const MAX_BADGES: usize = 6;

/// Clamps a float into an inclusive range.
///
/// # Arguments
///
/// * `value` — the value to clamp; may be any `f32`, including non-finite.
/// * `range` — the inclusive bounds.
///
/// # Returns
///
/// `value` constrained to `range`. Infinities clamp to the nearest bound.
/// NaN returns the range start, because NaN has no nearest bound and every
/// downstream consumer of this value is arithmetic that NaN would poison.
///
/// NaN is not reachable through the HTTP API — JSON has no literal for it —
/// so this branch exists to keep the function total for the property tests
/// rather than to handle real input.
///
/// # Examples
///
/// ```
/// use poster_service::spec::clamp::{self, BLUR_SIGMA};
///
/// assert_eq!(clamp::f32_into(24.0, BLUR_SIGMA), 24.0);
/// assert_eq!(clamp::f32_into(1000.0, BLUR_SIGMA), 96.0);
/// assert_eq!(clamp::f32_into(f32::NAN, BLUR_SIGMA), 0.0);
/// ```
#[must_use]
pub fn f32_into(value: f32, range: RangeInclusive<f32>) -> f32 {
    if value.is_nan() {
        return *range.start();
    }
    value.clamp(*range.start(), *range.end())
}

/// Clamps an unsigned integer into an inclusive range.
///
/// # Arguments
///
/// * `value` — the value to clamp.
/// * `range` — the inclusive bounds.
///
/// # Returns
///
/// `value` constrained to `range`.
#[must_use]
pub fn u32_into(value: u32, range: RangeInclusive<u32>) -> u32 {
    value.clamp(*range.start(), *range.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_at_both_ends() {
        assert!((f32_into(-5.0, BLUR_BAND_FRACTION) - 0.05).abs() < f32::EPSILON);
        assert!((f32_into(5.0, BLUR_BAND_FRACTION) - 0.60).abs() < f32::EPSILON);
        assert_eq!(u32_into(0, BADGE_HEIGHT), 24);
        assert_eq!(u32_into(9_999, BADGE_HEIGHT), 96);
    }

    #[test]
    fn infinities_clamp_to_the_nearest_bound() {
        assert!((f32_into(f32::INFINITY, DARKEN_STRENGTH) - 0.95).abs() < f32::EPSILON);
        assert!(f32_into(f32::NEG_INFINITY, DARKEN_STRENGTH).abs() < f32::EPSILON);
    }

    #[test]
    fn nan_resolves_to_the_range_start() {
        assert!((f32_into(f32::NAN, LOGO_WIDTH_FRACTION) - 0.10).abs() < f32::EPSILON);
    }

    #[test]
    fn every_range_is_non_empty() {
        // A range whose start exceeded its end would make f32::clamp panic at
        // runtime rather than fail here.
        for range in [
            BLUR_BAND_FRACTION,
            BLUR_SIGMA,
            DARKEN_STRENGTH,
            LOGO_WIDTH_FRACTION,
            LOGO_BOTTOM_FRACTION,
        ] {
            assert!(range.start() <= range.end(), "empty range: {range:?}");
        }
        assert!(BADGE_HEIGHT.start() <= BADGE_HEIGHT.end());
    }
}
