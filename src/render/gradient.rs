//! Alpha ramps for the blur band and the darkening layer.
//!
//! Both are vertical, single-channel, and computed per row rather than per
//! pixel: every pixel in a row shares an alpha, so computing it once and
//! reusing it across the row turns a per-pixel `powf` into a per-row one.

/// Returns the alpha ramp used to composite the blurred band.
///
/// The band's top edge must dissolve rather than show a seam, so alpha rises
/// from zero at the top of the feather to full within `feather` rows and stays
/// full to the bottom.
///
/// The ramp is smoothstep rather than linear. A linear ramp leaves a visible
/// Mach band at both ends — the eye is sensitive to discontinuities in the
/// *derivative*, not just the value, and smoothstep is C¹ continuous at both
/// endpoints where a linear ramp is not.
///
/// # Arguments
///
/// * `band_height` — height of the blurred band in pixels.
/// * `feather` — number of rows over which alpha rises from zero to one.
///
/// # Returns
///
/// One alpha per row, from the top of the band downward, each in `0..=255`.
///
/// # Examples
///
/// ```
/// use poster_service::render::gradient::band_alpha;
///
/// let ramp = band_alpha(100, 40);
/// assert_eq!(ramp.len(), 100);
/// assert_eq!(ramp[0], 0, "the band starts fully transparent");
/// assert_eq!(ramp[99], 255, "and is fully opaque by the bottom");
/// ```
#[must_use]
pub fn band_alpha(band_height: u32, feather: u32) -> Vec<u8> {
    (0..band_height)
        .map(|row| {
            if feather == 0 {
                return 255;
            }
            let t = (as_f32(row) / as_f32(feather)).min(1.0);
            to_u8(smoothstep(t))
        })
        .collect()
}

/// Converts a row count to `f32`.
///
/// Exact for every value this module sees: ramps span at most a few thousand
/// rows, far inside the 24-bit range `f32` represents without loss. Values
/// beyond that saturate rather than losing precision silently.
fn as_f32(value: u32) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Returns the alpha ramp for the darkening layer.
///
/// Rises from zero at the top of the band to `strength` at the bottom edge,
/// following a quadratic rather than a linear curve so the darkening is
/// concentrated where the logo and badges sit rather than being spread evenly
/// over artwork that does not need it.
///
/// # Arguments
///
/// * `band_height` — height over which the ramp is applied, in pixels.
/// * `strength` — peak opacity at the bottom edge, in `0.0..=1.0`.
///
/// # Returns
///
/// One alpha per row, from the top of the ramp downward.
#[must_use]
pub fn darken_alpha(band_height: u32, strength: f32) -> Vec<u8> {
    let strength = strength.clamp(0.0, 1.0);
    (0..band_height)
        .map(|row| {
            if band_height <= 1 {
                return to_u8(strength);
            }
            let t = as_f32(row) / as_f32(band_height - 1);
            to_u8(t * t * strength)
        })
        .collect()
}

/// Smoothstep: `3t² - 2t³`, clamped to `0..=1`.
///
/// Zero derivative at both endpoints, which is what removes the Mach banding a
/// linear ramp produces.
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Converts a `0.0..=1.0` fraction to `0..=255`, rounding.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_ramp_spans_the_full_range() {
        let ramp = band_alpha(200, 60);
        assert_eq!(ramp.len(), 200);
        assert_eq!(ramp[0], 0);
        assert_eq!(*ramp.last().expect("non-empty"), 255);
    }

    #[test]
    fn the_band_ramp_is_monotonic() {
        // A non-monotonic ramp would show as a bright line across the band.
        let ramp = band_alpha(300, 90);
        for pair in ramp.windows(2) {
            assert!(pair[1] >= pair[0], "ramp decreased: {pair:?}");
        }
    }

    #[test]
    fn the_band_ramp_reaches_full_opacity_by_the_feather_boundary() {
        let feather = 50_u32;
        let ramp = band_alpha(200, feather);

        assert_eq!(ramp[feather as usize], 255, "opaque at the boundary");
        assert!(
            ramp[(feather / 2) as usize] < 255,
            "still ramping at the midpoint"
        );

        // Not asserted at feather - 1: smoothstep is asymptotically flat
        // approaching 1, so its last few rows round to 255 before the
        // boundary. That flatness is the property the curve was chosen for,
        // so pinning the exact row it saturates would be pinning a rounding
        // artefact.
        assert!(ramp[..feather as usize].iter().any(|&a| a < 255));
    }

    #[test]
    fn a_zero_feather_is_fully_opaque_throughout() {
        // Degenerate but reachable: a band one pixel tall has nothing to
        // feather over.
        assert!(band_alpha(10, 0).iter().all(|&a| a == 255));
    }

    #[test]
    fn the_darkening_ramp_peaks_at_the_requested_strength() {
        let ramp = darken_alpha(100, 0.5);
        assert_eq!(ramp[0], 0);
        assert_eq!(*ramp.last().expect("non-empty"), to_u8(0.5));
    }

    #[test]
    fn the_darkening_ramp_is_monotonic() {
        for strength in [0.0, 0.25, 0.55, 0.95] {
            let ramp = darken_alpha(150, strength);
            for pair in ramp.windows(2) {
                assert!(pair[1] >= pair[0], "ramp decreased at strength {strength}");
            }
        }
    }

    #[test]
    fn the_darkening_ramp_is_concentrated_at_the_bottom() {
        // Quadratic rather than linear: at the halfway row the ramp should be
        // roughly a quarter of peak, not half. This is what keeps the
        // darkening off artwork that does not need it.
        let ramp = darken_alpha(101, 1.0);
        let midpoint = f32::from(ramp[50]) / 255.0;
        assert!(
            (midpoint - 0.25).abs() < 0.02,
            "midpoint was {midpoint}, expected about 0.25"
        );
    }

    #[test]
    fn degenerate_heights_do_not_panic() {
        assert!(band_alpha(0, 10).is_empty());
        assert!(darken_alpha(0, 0.5).is_empty());
        assert_eq!(darken_alpha(1, 0.5).len(), 1);
        assert_eq!(band_alpha(1, 10).len(), 1);
    }

    #[test]
    fn smoothstep_is_flat_at_both_ends() {
        // The property that removes Mach banding: the derivative approaches
        // zero at 0 and 1, so there is no visible discontinuity where the
        // ramp meets the unblurred image.
        let delta = 0.001;
        let slope_at_zero = (smoothstep(delta) - smoothstep(0.0)) / delta;
        let slope_at_one = (smoothstep(1.0) - smoothstep(1.0 - delta)) / delta;

        assert!(slope_at_zero < 0.01, "slope at 0 was {slope_at_zero}");
        assert!(slope_at_one < 0.01, "slope at 1 was {slope_at_one}");
    }

    #[test]
    fn strength_outside_the_unit_range_is_clamped() {
        assert_eq!(*darken_alpha(10, 5.0).last().expect("non-empty"), 255);
        assert_eq!(*darken_alpha(10, -5.0).last().expect("non-empty"), 0);
    }
}
