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
/// # Why there are two shapes
///
/// The band's shape decides what the band is *for*, and the two answers want
/// opposite curves. `saturate_at` selects between them.
///
/// `None` gives a quadratic: the ramp stays near zero for most of its run and
/// arrives late, so the band darkens the artwork immediately under the logo
/// and leaves the rest of it alone. That is a treatment applied to artwork.
///
/// `Some(fraction)` gives a smoothstep that reaches full opacity that far down
/// the band and holds: the artwork is gone well before the bottom edge, and
/// what remains is a coloured ground with the blurred artwork showing only
/// through the band's upper reaches. That is a surface the logo sits on, and
/// it is what the reference posters do.
///
/// Neither is a refinement of the other, so the preset names which one it
/// wants rather than the renderer picking.
///
/// # Why the early ramp is not simply a fractional exponent
///
/// `t.powf(0.4)` produces the same early arrival and was tried first. It has
/// infinite slope at `t = 0`, which draws a hard line across the poster where
/// the band begins — the seam the feather exists to prevent, reintroduced by
/// the ramp beside it. Smoothstep has zero slope at both ends, so it can climb
/// just as fast in the middle without an edge at either boundary.
///
/// # Arguments
///
/// * `band_height` — height over which the ramp is applied, in pixels.
/// * `strength` — peak opacity at the bottom edge, in `0.0..=1.0`.
/// * `saturate_at` — fraction of the band over which the ramp rises to full,
///   or `None` for the quadratic.
///
/// # Returns
///
/// One alpha per row, from the top of the ramp downward.
///
/// # Examples
///
/// ```
/// use poster_service::render::gradient::darken_alpha;
///
/// // A quadratic ramp is a quarter of the way up at its midpoint.
/// let late = darken_alpha(101, 1.0, None);
/// assert_eq!(late[50], 64);
///
/// // A ramp saturating at 65% is most of the way up by the same row.
/// let early = darken_alpha(101, 1.0, Some(0.65));
/// assert!(early[50] > 210);
/// assert_eq!(early[80], 255, "and fully arrived past its saturation point");
/// ```
#[must_use]
pub fn darken_alpha(band_height: u32, strength: f32, saturate_at: Option<f32>) -> Vec<u8> {
    let strength = strength.clamp(0.0, 1.0);
    (0..band_height)
        .map(|row| {
            if band_height <= 1 {
                return to_u8(strength);
            }
            let t = as_f32(row) / as_f32(band_height - 1);
            let shaped = match saturate_at {
                // A non-positive or non-finite fraction would divide the ramp
                // into a step at the band's top edge, so it falls back to the
                // quadratic rather than drawing a hard line.
                Some(point) if point.is_finite() && point > 0.0 => smoothstep((t / point).min(1.0)),
                _ => t * t,
            };
            to_u8(shaped * strength)
        })
        .collect()
}

/// Returns the alpha ramp for the inset shadow along the top edge.
///
/// Peaks at `strength` on the very first row and falls linearly to zero over
/// `extent` rows.
///
/// Linear here, where the band ramps quadratically or smoothly, because this
/// one does a different job. The band's job is to darken the artwork under the
/// logo; this one's is to seat the badge on a consistently dark ground
/// whatever the artwork behind it, which needs its weight at the top and a
/// steady release over a long run. A curve would concentrate the shadow near
/// one end and leave a visible boundary at the other.
///
/// # Arguments
///
/// * `extent` — number of rows the shadow spans, from the top edge down.
/// * `strength` — opacity on the top row, in `0.0..=1.0`.
///
/// # Returns
///
/// One alpha per row, from the top edge downward.
///
/// # Examples
///
/// ```
/// use poster_service::render::gradient::inset_top_alpha;
///
/// let ramp = inset_top_alpha(100, 0.8);
/// assert_eq!(ramp[0], 204, "darkest against the top edge");
/// assert_eq!(ramp[99], 0, "and gone by the end of its extent");
/// ```
#[must_use]
pub fn inset_top_alpha(extent: u32, strength: f32) -> Vec<u8> {
    let strength = strength.clamp(0.0, 1.0);
    (0..extent)
        .map(|row| {
            if extent <= 1 {
                return to_u8(strength);
            }
            let t = as_f32(row) / as_f32(extent - 1);
            to_u8((1.0 - t) * strength)
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
        let ramp = darken_alpha(100, 0.5, None);
        assert_eq!(ramp[0], 0);
        assert_eq!(*ramp.last().expect("non-empty"), to_u8(0.5));
    }

    #[test]
    fn the_darkening_ramp_is_monotonic() {
        for strength in [0.0, 0.25, 0.55, 0.95] {
            let ramp = darken_alpha(150, strength, None);
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
        let ramp = darken_alpha(101, 1.0, None);
        let midpoint = f32::from(ramp[50]) / 255.0;
        assert!(
            (midpoint - 0.25).abs() < 0.02,
            "midpoint was {midpoint}, expected about 0.25"
        );
    }

    #[test]
    fn degenerate_heights_do_not_panic() {
        assert!(band_alpha(0, 10).is_empty());
        assert!(darken_alpha(0, 0.5, None).is_empty());
        assert_eq!(darken_alpha(1, 0.5, None).len(), 1);
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
        assert_eq!(*darken_alpha(10, 5.0, None).last().expect("non-empty"), 255);
        assert_eq!(*darken_alpha(10, -5.0, None).last().expect("non-empty"), 0);
    }

    #[test]
    fn a_saturating_ramp_arrives_early_and_holds() {
        // The property that separates the reference treatment from the
        // original one: most of the tint is laid down in the first half of
        // the band rather than the last, and it is complete before the edge.
        let early = darken_alpha(101, 1.0, Some(0.65));
        let late = darken_alpha(101, 1.0, None);
        assert!(early[50] > late[50] * 2, "{} vs {}", early[50], late[50]);
        assert_eq!(early[0], 0, "both still start transparent");
        assert_eq!(early[70], 255, "saturated past its own point");
        assert_eq!(*early.last().expect("non-empty"), 255);
    }

    #[test]
    fn a_saturating_ramp_has_no_hard_edge_at_the_band_top() {
        // The regression a fractional exponent introduced: an infinite slope
        // at the top of the band draws a line across the poster.
        let ramp = darken_alpha(300, 1.0, Some(0.65));
        let first_step = i32::from(ramp[1]) - i32::from(ramp[0]);
        let steepest = ramp
            .windows(2)
            .map(|pair| i32::from(pair[1]) - i32::from(pair[0]))
            .max()
            .expect("non-empty");
        assert!(
            first_step <= 1,
            "the ramp starts with a jump of {first_step}"
        );
        assert!(steepest < 6, "the ramp has a step of {steepest} somewhere");
    }

    #[test]
    fn a_malformed_saturation_point_falls_back_to_the_quadratic() {
        let quadratic = darken_alpha(50, 0.9, None);
        for broken in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                darken_alpha(50, 0.9, Some(broken)),
                quadratic,
                "saturation point {broken}"
            );
        }
    }

    #[test]
    fn the_darkening_ramp_is_monotonic_for_either_shape() {
        for saturate_at in [None, Some(0.3), Some(0.65), Some(1.0)] {
            let ramp = darken_alpha(150, 0.9, saturate_at);
            for pair in ramp.windows(2) {
                assert!(pair[1] >= pair[0], "ramp decreased at {saturate_at:?}");
            }
        }
    }

    #[test]
    fn the_inset_shadow_is_strongest_at_the_top_edge() {
        let ramp = inset_top_alpha(100, 0.85);
        assert_eq!(ramp.len(), 100);
        assert_eq!(ramp[0], to_u8(0.85));
        assert_eq!(*ramp.last().expect("non-empty"), 0);
    }

    #[test]
    fn the_inset_shadow_decreases_monotonically() {
        let ramp = inset_top_alpha(180, 0.85);
        for pair in ramp.windows(2) {
            assert!(pair[1] <= pair[0], "ramp increased: {pair:?}");
        }
    }

    #[test]
    fn the_inset_shadow_is_linear() {
        // Half the alpha at the halfway row is what separates this ramp from
        // the quadratic one the band uses.
        let ramp = inset_top_alpha(101, 1.0);
        let midpoint = f32::from(ramp[50]) / 255.0;
        assert!(
            (midpoint - 0.5).abs() < 0.02,
            "midpoint was {midpoint}, expected about 0.5"
        );
    }

    #[test]
    fn a_zero_strength_shadow_is_a_no_op() {
        assert!(inset_top_alpha(50, 0.0).iter().all(|&a| a == 0));
    }

    #[test]
    fn degenerate_shadow_extents_do_not_panic() {
        assert!(inset_top_alpha(0, 0.85).is_empty());
        assert_eq!(inset_top_alpha(1, 0.85), vec![to_u8(0.85)]);
    }
}
