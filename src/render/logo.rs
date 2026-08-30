//! Title logo fitting and placement.

use crate::render::source::{self, Rgba8};
use crate::render::RenderError;

/// Maximum logo height, as a fraction of poster height.
///
/// `logo_width_fraction` alone under-constrains an unusually tall logo: a
/// narrow, very tall wordmark scaled to 60% of the poster width would run off
/// the top of the frame. Fitting inside both bounds is what keeps the logo on
/// the poster whatever its aspect ratio.
pub const MAX_HEIGHT_FRACTION: f32 = 0.20;

/// Where a logo should be drawn, and at what size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Left edge in pixels.
    pub x: i32,
    /// Top edge in pixels.
    pub y: i32,
    /// Drawn width in pixels.
    pub width: u32,
    /// Drawn height in pixels.
    pub height: u32,
}

/// Computes the placement of a logo within a poster.
///
/// The logo is fitted inside both the requested width fraction and
/// [`MAX_HEIGHT_FRACTION`], centred horizontally, and anchored so its
/// *bottom* edge sits `bottom_fraction` above the poster's bottom edge.
///
/// Anchoring the bottom rather than the top is what keeps a row of posters
/// looking aligned: logos differ in height, so a top anchor would leave their
/// baselines at different distances from the frame edge.
///
/// # Arguments
///
/// * `poster` — poster dimensions, width then height.
/// * `logo` — intrinsic logo dimensions, width then height.
/// * `width_fraction` — target width as a fraction of poster width.
/// * `bottom_fraction` — gap below the logo as a fraction of poster height.
///
/// # Returns
///
/// The placement, or `None` if either input has a zero dimension.
///
/// # Examples
///
/// ```
/// use poster_service::render::logo;
///
/// // A wide logo is limited by the width fraction.
/// let placed = logo::place((1000, 1500), (800, 200), 0.6, 0.12).expect("placeable");
/// assert_eq!(placed.width, 600);
/// // Centred horizontally.
/// assert_eq!(placed.x, 200);
/// ```
#[must_use]
pub fn place(
    poster: (u32, u32),
    logo: (u32, u32),
    width_fraction: f32,
    bottom_fraction: f32,
) -> Option<Placement> {
    let (poster_width, poster_height) = poster;
    let (logo_width, logo_height) = logo;
    if poster_width == 0 || poster_height == 0 || logo_width == 0 || logo_height == 0 {
        return None;
    }

    // Geometry in f64 rather than f32: every u32 converts exactly, so there is
    // no precision question to reason about, and the arithmetic here runs once
    // per render rather than once per pixel.
    let poster_width_f = f64::from(poster_width);
    let poster_height_f = f64::from(poster_height);

    let target_width = poster_width_f * f64::from(width_fraction);
    let max_height = poster_height_f * f64::from(MAX_HEIGHT_FRACTION);

    let scale_for_width = target_width / f64::from(logo_width);
    let scale_for_height = max_height / f64::from(logo_height);
    // The smaller scale wins, so the logo satisfies both bounds rather than
    // whichever was checked last.
    let scale = scale_for_width.min(scale_for_height);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = ((f64::from(logo_width) * scale).round() as u32).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = ((f64::from(logo_height) * scale).round() as u32).max(1);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bottom_gap = (poster_height_f * f64::from(bottom_fraction)).round() as u32;

    #[allow(clippy::cast_possible_wrap)]
    let x = ((poster_width - width.min(poster_width)) / 2) as i32;
    #[allow(clippy::cast_possible_wrap)]
    let y = poster_height
        .saturating_sub(bottom_gap)
        .saturating_sub(height) as i32;

    Some(Placement {
        x,
        y,
        width,
        height,
    })
}

/// Resizes a logo to its placement size.
///
/// # Errors
///
/// [`RenderError::Resize`] if the resampler rejects the dimensions.
pub fn fit(logo: &Rgba8, placement: Placement) -> Result<Rgba8, RenderError> {
    source::resize(logo, placement.width, placement.height)
}

#[cfg(test)]
mod tests {
    // Fixture casts throughout: dimensions are small literals, so nothing the
    // lints warn about can occur.
    #![allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]

    use super::*;

    const POSTER: (u32, u32) = (1000, 1500);

    #[test]
    fn a_wide_logo_is_limited_by_the_width_fraction() {
        let placed = place(POSTER, (800, 200), 0.6, 0.12).expect("placeable");
        assert_eq!(placed.width, 600);
        assert_eq!(placed.height, 150);
    }

    #[test]
    fn a_tall_logo_is_limited_by_the_height_cap() {
        // Without the height cap this would be 600 wide and 1200 tall, which
        // runs off the top of a 1500 px frame.
        let placed = place(POSTER, (200, 400), 0.6, 0.12).expect("placeable");
        let max_height = (1500.0 * MAX_HEIGHT_FRACTION) as u32;

        assert!(
            placed.height <= max_height,
            "logo is {} tall, cap is {max_height}",
            placed.height
        );
        assert!(placed.width < 600, "width should have been reduced too");
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        for logo in [(800, 200), (200, 400), (500, 500), (1920, 137)] {
            let placed = place(POSTER, logo, 0.6, 0.12).expect("placeable");
            let original = f64::from(logo.0) / f64::from(logo.1);
            let drawn = f64::from(placed.width) / f64::from(placed.height);
            assert!(
                (original - drawn).abs() / original < 0.02,
                "aspect drifted for {logo:?}: {original} -> {drawn}"
            );
        }
    }

    #[test]
    fn the_logo_is_centred_horizontally() {
        let placed = place(POSTER, (800, 200), 0.6, 0.12).expect("placeable");
        let left = placed.x;
        #[allow(clippy::cast_possible_wrap)]
        let right = (POSTER.0 - placed.width) as i32 - left;
        assert_eq!(left, right, "logo is not centred");
    }

    #[test]
    fn the_bottom_edge_sits_at_the_requested_gap() {
        // Bottom-anchored, so logos of different heights align at the base.
        let bottom_fraction: f32 = 0.12;
        let expected_gap = (1500.0 * bottom_fraction).round() as u32;

        for logo in [(800, 200), (600, 100), (400, 300)] {
            let placed = place(POSTER, logo, 0.6, bottom_fraction).expect("placeable");
            #[allow(clippy::cast_sign_loss)]
            let gap = POSTER.1 - (placed.y as u32 + placed.height);
            assert_eq!(gap, expected_gap, "gap differed for {logo:?}");
        }
    }

    #[test]
    fn the_logo_stays_inside_the_frame() {
        for logo in [(800, 200), (200, 900), (3000, 50), (10, 10)] {
            for width_fraction in [0.1, 0.5, 0.9] {
                for bottom_fraction in [0.02, 0.2, 0.4] {
                    let placed =
                        place(POSTER, logo, width_fraction, bottom_fraction).expect("placeable");
                    assert!(placed.x >= 0, "logo runs off the left for {logo:?}");
                    assert!(placed.y >= 0, "logo runs off the top for {logo:?}");
                    #[allow(clippy::cast_sign_loss)]
                    {
                        assert!(placed.x as u32 + placed.width <= POSTER.0);
                        assert!(placed.y as u32 + placed.height <= POSTER.1);
                    }
                }
            }
        }
    }

    #[test]
    fn a_zero_dimension_is_not_placeable() {
        assert!(place(POSTER, (0, 200), 0.6, 0.12).is_none());
        assert!(place(POSTER, (800, 0), 0.6, 0.12).is_none());
        assert!(place((0, 1500), (800, 200), 0.6, 0.12).is_none());
    }

    #[test]
    fn a_degenerate_logo_still_yields_at_least_one_pixel() {
        // A 1x1000 logo scaled to fit produces a sub-pixel width, which must
        // clamp to 1 rather than to 0 and a zero-sized resize.
        let placed = place(POSTER, (1, 1000), 0.6, 0.12).expect("placeable");
        assert!(placed.width >= 1 && placed.height >= 1);
    }

    #[test]
    fn placement_scales_with_the_poster() {
        let small = place((1000, 1500), (800, 200), 0.6, 0.12).expect("placeable");
        let large = place((2000, 3000), (800, 200), 0.6, 0.12).expect("placeable");

        assert_eq!(large.width, small.width * 2);
        assert_eq!(large.x, small.x * 2);
    }
}
