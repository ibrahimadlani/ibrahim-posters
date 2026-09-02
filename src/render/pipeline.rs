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
use crate::render::{badges, blur, caption, encode, gradient, logo, palette, source, RenderError};
use crate::spec::{ResolvedSpec, Rgb};

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

/// Clearance kept between the caption and the logo above it, as a multiple of
/// the caption's text size.
///
/// The logo is lifted by this much when a caption is present rather than the
/// preset carrying two logo positions, because a preset describes one layout
/// and "where the logo sits" should not depend on which of them a caller
/// happens to be reading.
const CAPTION_LOGO_CLEARANCE: f32 = 1.5;

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

    // 5. Tint the band for legibility.
    if band_height > 0 && spec.darken_strength > 0.0 {
        apply_band_tint(
            &mut canvas,
            band_height,
            spec.darken_strength,
            spec.band_colour,
            spec.band_ramp_fraction,
        );
    }

    // 6. Seat the badge on a predictable ground. Read the badge colour from
    // the artwork *before* this runs: the shadow is part of the treatment, and
    // sampling after it would darken the badge in step with the shadow instead
    // of matching the poster.
    let badge_palette = spec
        .badge_from_artwork
        .then(|| palette::from_artwork(&canvas));

    let shadow_extent = fraction_of(height, spec.top_shadow_fraction);
    if shadow_extent > 0 && spec.top_shadow_strength > 0.0 {
        apply_inset_top_shadow(&mut canvas, shadow_extent, spec.top_shadow_strength);
    }

    // 7-8. Composite the sharp elements.
    let mut pixmap = source::to_pixmap(&canvas)?;
    if let Some(bytes) = &assets.logo {
        draw_logo(&mut pixmap, bytes, spec, logo_bottom_fraction(spec, height))?;
    }
    draw_caption(&mut pixmap, spec)?;
    draw_badges(&mut pixmap, spec, badge_palette)?;

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

/// Blends the bottom band toward `colour` along the darkening ramp.
///
/// # Why a colour and not just black
///
/// Blurring and darkening alone leave the band a desaturated version of
/// whatever happened to be at the bottom of the artwork, which differs from
/// poster to poster and from a wall of posters reads as inconsistency rather
/// than as variety. Blending toward a fixed colour gives every poster the same
/// footing while the blur keeps the artwork's own shapes visible through it.
///
/// Black is the identity here: blending toward `#000000` is exactly the
/// multiply this function used to perform, so a preset that names no colour
/// renders as it always has.
///
/// Same bound as the blend above: `channel * keep + tint * alpha` with the two
/// weights summing to 255 cannot exceed `255 * 255`, so the quotient is at
/// most 255.
#[allow(clippy::cast_possible_truncation)]
fn apply_band_tint(
    canvas: &mut Rgba8,
    band_height: u32,
    strength: f32,
    colour: Rgb,
    saturate_at: Option<f32>,
) {
    let stride = canvas.width as usize * 4;
    let top = canvas.height.saturating_sub(band_height);
    let ramp = gradient::darken_alpha(band_height, strength, saturate_at);
    let tint = colour.to_bytes();

    for (row, alpha) in ramp.iter().enumerate() {
        let weight = u32::from(*alpha);
        let keep = 255 - weight;
        let row_start = (top as usize + row) * stride;
        for pixel in canvas.pixels[row_start..row_start + stride]
            .as_chunks_mut::<4>()
            .0
        {
            // Colour channels only; alpha is untouched because the background
            // is opaque and must stay so.
            for (channel, tint) in pixel[..3].iter_mut().zip(tint) {
                *channel =
                    ((u32::from(*channel) * keep + u32::from(tint) * weight + 127) / 255) as u8;
            }
        }
    }
}

/// Darkens the artwork under the top edge, strongest at the edge itself.
///
/// The badge sits at the top of the poster over artwork the service does not
/// choose, so its legibility would otherwise depend on whatever the title's
/// designer put there. This ramp makes the ground behind it predictable
/// without hiding the artwork: it is gone well before the halfway line.
///
/// Same bound as the tint above.
#[allow(clippy::cast_possible_truncation)]
fn apply_inset_top_shadow(canvas: &mut Rgba8, extent: u32, strength: f32) {
    let stride = canvas.width as usize * 4;
    let extent = extent.min(canvas.height);
    let ramp = gradient::inset_top_alpha(extent, strength);

    for (row, alpha) in ramp.iter().enumerate() {
        let keep = u32::from(255 - *alpha);
        let row_start = row * stride;
        for pixel in canvas.pixels[row_start..row_start + stride]
            .as_chunks_mut::<4>()
            .0
        {
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
    bottom_fraction: f32,
) -> Result<(), RenderError> {
    let decoded = source::decode(bytes)?;
    let Some(placement) = logo::place(
        spec.dimensions(),
        (decoded.width, decoded.height),
        spec.logo_width_fraction,
        bottom_fraction,
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

/// Returns how far above the bottom edge the logo sits.
///
/// Without a caption this is the preset's own value. With one, the logo is
/// raised enough to clear the caption, so the two never overlap on a poster
/// whose logo happens to be tall.
///
/// Dimensions are at most 3000 and the caption size at most 320, so both
/// convert to `f32` exactly.
#[allow(clippy::cast_precision_loss)]
fn logo_bottom_fraction(spec: &ResolvedSpec, height: u32) -> f32 {
    if spec.caption.is_none() || height == 0 {
        return spec.logo_bottom_fraction;
    }
    let clearance = CAPTION_LOGO_CLEARANCE * spec.caption_height as f32 / height as f32;
    spec.logo_bottom_fraction
        .max(spec.caption_bottom_fraction + clearance)
}

/// Rasterises and composites the genre and rating line.
///
/// Drawn after the logo and before the badge, which is only a matter of
/// overlap order; the three occupy separate bands of the poster by
/// construction.
///
/// The caption size is at most 320 px and poster dimensions at most 3000, so
/// the conversions are exact. The placement is clamped into the frame before
/// it is used.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn draw_caption(pixmap: &mut tiny_skia::Pixmap, spec: &ResolvedSpec) -> Result<(), RenderError> {
    let Some(caption) = &spec.caption else {
        return Ok(());
    };

    let (width, height) = spec.dimensions();
    let Some(strip) = caption::rasterise(
        &caption.line(),
        spec.caption_height as f32,
        spec.caption_colour,
    )?
    else {
        return Ok(());
    };

    let centre = height as f32 * (1.0 - spec.caption_bottom_fraction);
    let x = ((width as i32 - strip.width() as i32) / 2).max(0);
    let y = (centre - strip.height() as f32 / 2.0).max(0.0) as i32;

    pixmap.draw_pixmap(
        x,
        y,
        strip.as_ref(),
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
fn draw_badges(
    pixmap: &mut tiny_skia::Pixmap,
    spec: &ResolvedSpec,
    derived: Option<palette::BadgePalette>,
) -> Result<(), RenderError> {
    if spec.badges.is_empty() {
        return Ok(());
    }

    let (width, height) = spec.dimensions();
    // Zero means "size each badge to its own text", which is what the
    // fraction's floor encodes, so it maps to no fixed width at all.
    let fixed =
        (spec.badge_width_fraction > 0.0).then_some(width as f32 * spec.badge_width_fraction);
    let laid_out = badges::layout(&spec.badges, spec.badge_height as f32, fixed)?;
    let Some(row) = badges::rasterise(&laid_out, derived)? else {
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
    let y = (height as f32 * spec.badge_top_fraction) as i32;

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
