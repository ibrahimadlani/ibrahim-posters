//! Stage sequencing.
//!
//! The order below is not arbitrary; three constraints fix most of it.
//!
//! - **Resize first.** Everything after it works at output resolution, so no
//!   stage pays for pixels that will be discarded.
//! - **Darken after blur.** Applying the ramp first would have the blur smear
//!   the darkening upward past the band's edge, softening the boundary the
//!   feather exists to control.
//! - **Logo and badges last.** They are the sharpest elements on the poster
//!   and must not be touched by either the blur or the ramp.

use tiny_skia::{PixmapPaint, Transform};

use crate::render::source::Rgba8;
use crate::render::{badges, blur, encode, gradient, logo, source, RenderError};
use crate::spec::ResolvedSpec;

/// Assets a render consumes, already fetched.
///
/// Byte buffers rather than paths or handles: this is what keeps `render/`
/// free of I/O, and therefore unit-testable and pixel-comparable across
/// versions.
#[derive(Debug, Clone, Default)]
pub struct Assets {
    /// Encoded background artwork.
    pub background: Vec<u8>,
    /// Encoded title logo, if the specification asks for one.
    pub logo: Option<Vec<u8>>,
}

/// Fraction of the band height over which the blur fades in at its top edge.
const FEATHER_RATIO: f32 = 0.55;

/// Distance from the top edge to the badge row, as a fraction of height.
const BADGE_TOP_FRACTION: f32 = 0.045;

/// Renders a poster to straight-alpha RGBA8.
///
/// Returns pixels rather than encoded bytes so the visual regression suite can
/// compare pre-encode buffers. libwebp output is not bit-stable across library
/// versions, so comparing encoded bytes would fail CI on a libwebp bump that
/// changed nothing visible.
///
/// # Arguments
///
/// * `spec` — a resolved, clamped specification.
/// * `assets` — the encoded artwork the specification refers to.
///
/// # Returns
///
/// The finished poster at the specification's output size.
///
/// # Errors
///
/// See [`RenderError`]. Every variant reflects bad input rather than a
/// transient condition, so none is worth retrying.
pub fn render(spec: &ResolvedSpec, assets: &Assets) -> Result<Rgba8, RenderError> {
    let (width, height) = spec.dimensions();

    // 1-2. Decode and resize to fill the frame.
    let decoded = source::decode(&assets.background)?;
    let mut canvas = source::resize_to_fill(&decoded, width, height)?;

    // 3-4. Blur the bottom band and composite it back through a feathered ramp.
    let band_height = fraction_of(height, spec.blur_band_fraction);
    if band_height > 0 && spec.blur_sigma > 0.0 {
        apply_blur_band(&mut canvas, band_height, spec.blur_sigma);
    }

    // 5. Darken for legibility, over the same band.
    if band_height > 0 && spec.darken_strength > 0.0 {
        apply_darkening(&mut canvas, band_height, spec.darken_strength);
    }

    // 6-7. Composite the sharp elements.
    let mut pixmap = source::to_pixmap(&canvas)?;
    if let Some(bytes) = &assets.logo {
        draw_logo(&mut pixmap, bytes, spec)?;
    }
    draw_badges(&mut pixmap, spec)?;

    source::from_pixmap(&pixmap)
}

/// Renders and encodes in one call.
///
/// # Errors
///
/// See [`RenderError`].
pub fn render_encoded(spec: &ResolvedSpec, assets: &Assets) -> Result<Vec<u8>, RenderError> {
    let poster = render(spec, assets)?;
    encode::webp(&poster, encode::DELIVERY_QUALITY)
}

/// Blurs the bottom band and composites it back through a feathered ramp.
///
/// Casts are allowed with their bounds stated. `downscale` and `band_rows` are
/// pixel counts bounded by the 8000 px source guard, so they convert to `f32`
/// exactly. The composite blends two `u8` samples with weights summing to 255,
/// so `(a * (255 - w) + b * w + 127) / 255` is at most 255 by construction —
/// a checked conversion there would branch once per channel per pixel of the
/// band to exclude a case the arithmetic already does.
///
/// The band is downscaled before blurring and upscaled afterwards. Sigma is
/// divided by the same factor, because a radius measured in pixels means
/// something different at a different resolution — omitting that division is
/// the mistake that makes a downscaled blur look identical to an undownscaled
/// one and hides the whole optimisation.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_blur_band(canvas: &mut Rgba8, band_height: u32, sigma: f32) {
    let width = canvas.width;
    let top = canvas.height.saturating_sub(band_height);
    let stride = width as usize * 4;
    let start = top as usize * stride;

    let mut band = canvas.pixels[start..].to_vec();
    let band_rows = u32::try_from(band.len() / stride).unwrap_or(u32::MAX);
    if band_rows == 0 {
        return;
    }

    let downscale = if sigma >= blur::MIN_SIGMA_FOR_DOWNSCALE {
        blur::DOWNSCALE
    } else {
        1
    };

    if downscale > 1 {
        let small_width = (width / downscale).max(1);
        let small_height = (band_rows / downscale).max(1);

        let Ok(band_image) = Rgba8::new(band.clone(), width, band_rows) else {
            return;
        };
        let Ok(mut small) = source::resize(&band_image, small_width, small_height) else {
            return;
        };

        blur::blur_rgba(
            &mut small.pixels,
            small_width,
            small_height,
            sigma / downscale as f32,
        );

        let Ok(restored) = source::resize(&small, width, band_rows) else {
            return;
        };
        band = restored.pixels;
    } else {
        blur::blur_rgba(&mut band, width, band_rows, sigma);
    }

    let feather = (band_rows as f32 * FEATHER_RATIO) as u32;
    let ramp = gradient::band_alpha(band_rows, feather);

    for (row, alpha) in ramp.iter().enumerate() {
        let alpha = u32::from(*alpha);
        let row_start = start + row * stride;
        let band_start = row * stride;
        for column in 0..stride {
            let original = u32::from(canvas.pixels[row_start + column]);
            let blurred = u32::from(band[band_start + column]);
            // Lerp toward the blurred value by the ramp's alpha.
            canvas.pixels[row_start + column] =
                ((original * (255 - alpha) + blurred * alpha + 127) / 255) as u8;
        }
    }
}

/// Applies the darkening ramp in place over the bottom band.
///
/// Same bound as the blend above: `channel * keep / 255` with `keep <= 255`
/// cannot exceed 255.
#[allow(clippy::cast_possible_truncation)]
fn apply_darkening(canvas: &mut Rgba8, band_height: u32, strength: f32) {
    let stride = canvas.width as usize * 4;
    let top = canvas.height.saturating_sub(band_height);
    let ramp = gradient::darken_alpha(band_height, strength);

    for (row, alpha) in ramp.iter().enumerate() {
        let keep = u32::from(255 - *alpha);
        let row_start = (top as usize + row) * stride;
        for pixel in canvas.pixels[row_start..row_start + stride]
            .as_chunks_mut::<4>()
            .0
        {
            // Multiply the colour channels toward black; alpha is untouched
            // because the background is opaque and must stay so.
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * keep + 127) / 255) as u8;
            }
        }
    }
}

/// Decodes, fits and composites the title logo.
fn draw_logo(
    pixmap: &mut tiny_skia::Pixmap,
    bytes: &[u8],
    spec: &ResolvedSpec,
) -> Result<(), RenderError> {
    let decoded = source::decode(bytes)?;
    let Some(placement) = logo::place(
        spec.dimensions(),
        (decoded.width, decoded.height),
        spec.logo_width_fraction,
        spec.logo_bottom_fraction,
    ) else {
        return Ok(());
    };

    let fitted = logo::fit(&decoded, placement)?;
    let logo_pixmap = source::to_pixmap(&fitted)?;

    pixmap.draw_pixmap(
        placement.x,
        placement.y,
        logo_pixmap.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Ok(())
}

/// Lays out and composites the badge row.
///
/// `badge_height` is clamped to at most 192 and poster height to 3000, so both
/// convert to `f32` exactly.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn draw_badges(pixmap: &mut tiny_skia::Pixmap, spec: &ResolvedSpec) -> Result<(), RenderError> {
    if spec.badges.is_empty() {
        return Ok(());
    }

    let (width, height) = spec.dimensions();
    let laid_out = badges::layout(&spec.badges, spec.badge_height as f32)?;
    let Some(row) = badges::rasterise(&laid_out)? else {
        return Ok(());
    };

    // Centred horizontally. A row wider than the poster is clipped rather
    // than scaled: scaling would make one poster's badges a different size
    // from another's, and the row is capped upstream by MAX_BADGES.
    #[allow(clippy::cast_possible_wrap)]
    let x = ((width as i32 - row.width() as i32) / 2).max(0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )]
    let y = (height as f32 * BADGE_TOP_FRACTION) as i32;

    pixmap.draw_pixmap(
        x,
        y,
        row.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Ok(())
}

/// Multiplies a dimension by a fraction, rounding down to whole pixels.
///
/// Dimensions are at most 3000 and fractions are clamped to `0.0..=1.0`, so
/// the conversions are exact in both directions.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn fraction_of(value: u32, fraction: f32) -> u32 {
    (value as f32 * fraction) as u32
}
