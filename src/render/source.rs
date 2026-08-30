//! Source artwork decoding and resizing.
//!
//! Two decoders are involved. `zune-jpeg` handles JPEG, which is the format
//! nearly every TMDB source arrives in; the `image` crate handles PNG and
//! WebP, with its own JPEG decoder disabled so the binary carries only one.
//!
//! Resizing happens before every other stage. Everything downstream then
//! works at output resolution, so no stage pays for pixels that will be
//! discarded — and at w1000 that is the difference between compositing over
//! 1.5 megapixels and compositing over whatever TMDB happened to serve.

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::render::RenderError;

/// Decoded artwork as straight-alpha RGBA8.
///
/// Straight rather than premultiplied because resizing premultiplied data and
/// then unpremultiplying amplifies error in near-transparent pixels — visible
/// as a dark fringe around a logo's antialiased edge. The conversion to
/// premultiplied happens once, after resizing, in [`to_pixmap`].
#[derive(Debug, Clone)]
pub struct Rgba8 {
    /// Row-major RGBA samples, four bytes per pixel.
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Rgba8 {
    /// Builds an image from raw samples.
    ///
    /// # Errors
    ///
    /// [`RenderError::MalformedBuffer`] if the buffer length does not match
    /// the stated dimensions.
    pub fn new(pixels: Vec<u8>, width: u32, height: u32) -> Result<Self, RenderError> {
        let expected = (width as usize) * (height as usize) * 4;
        if pixels.len() != expected {
            return Err(RenderError::MalformedBuffer {
                expected,
                found: pixels.len(),
            });
        }
        Ok(Self {
            pixels,
            width,
            height,
        })
    }
}

/// Decodes JPEG, PNG or WebP into straight-alpha RGBA8.
///
/// # Arguments
///
/// * `bytes` — the complete encoded file.
///
/// # Errors
///
/// [`RenderError::Decode`] if the format is unrecognised or the data is
/// corrupt.
pub fn decode(bytes: &[u8]) -> Result<Rgba8, RenderError> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return decode_jpeg(bytes);
    }
    decode_via_image_crate(bytes)
}

/// Decodes a JPEG with `zune-jpeg`.
fn decode_jpeg(bytes: &[u8]) -> Result<Rgba8, RenderError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder =
        zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    let pixels = decoder
        .decode()
        .map_err(|error| RenderError::Decode(error.to_string()))?;

    let info = decoder
        .info()
        .ok_or_else(|| RenderError::Decode("jpeg carried no dimensions".to_owned()))?;

    Rgba8::new(pixels, u32::from(info.width), u32::from(info.height))
}

/// Decodes PNG or WebP with the `image` crate.
fn decode_via_image_crate(bytes: &[u8]) -> Result<Rgba8, RenderError> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| RenderError::Decode(error.to_string()))?
        .into_rgba8();
    let (width, height) = (decoded.width(), decoded.height());

    Rgba8::new(decoded.into_raw(), width, height)
}

/// Resizes to exactly `width` by `height`.
///
/// Uses Lanczos3, dispatched to SIMD at runtime by `fast_image_resize`.
/// Lanczos3 rather than a cheaper filter because the background is the whole
/// image: resampling artefacts there are not hidden by anything.
///
/// # Errors
///
/// [`RenderError::Resize`] if the resampler rejects the dimensions.
pub fn resize(source: &Rgba8, width: u32, height: u32) -> Result<Rgba8, RenderError> {
    if source.width == width && source.height == height {
        return Ok(source.clone());
    }
    if width == 0 || height == 0 {
        return Err(RenderError::Resize("target dimension is zero".to_owned()));
    }

    // from_slice_u8 takes a mutable borrow even for a read-only source, so the
    // copy has to outlive the resize rather than being an inline temporary.
    let mut scratch = source.pixels.clone();
    let src = Image::from_slice_u8(source.width, source.height, &mut scratch, PixelType::U8x4)
        .map_err(|error| RenderError::Resize(error.to_string()))?;
    let mut dst = Image::new(width, height, PixelType::U8x4);

    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .map_err(|error| RenderError::Resize(error.to_string()))?;

    Rgba8::new(dst.into_vec(), width, height)
}

/// Resizes to fill `width` by `height`, cropping the overflow centrally.
///
/// Poster output is a fixed 2:3 frame and sources are not. Fitting inside the
/// frame would letterbox; filling and cropping keeps the frame covered, which
/// is what a poster needs. The crop is centred because the subject of a
/// poster or backdrop is centred far more often than it is not.
///
/// # Errors
///
/// [`RenderError::Resize`] if the resampler rejects the dimensions.
pub fn resize_to_fill(source: &Rgba8, width: u32, height: u32) -> Result<Rgba8, RenderError> {
    if source.width == 0 || source.height == 0 {
        return Err(RenderError::Resize("source dimension is zero".to_owned()));
    }

    let scale = f64::from(width) / f64::from(source.width);
    let scale = scale.max(f64::from(height) / f64::from(source.height));

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled_width = ((f64::from(source.width) * scale).ceil() as u32).max(width);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled_height = ((f64::from(source.height) * scale).ceil() as u32).max(height);

    let scaled = resize(source, scaled_width, scaled_height)?;
    crop_centred(&scaled, width, height)
}

/// Crops `source` to `width` by `height`, centred.
fn crop_centred(source: &Rgba8, width: u32, height: u32) -> Result<Rgba8, RenderError> {
    if source.width < width || source.height < height {
        return Err(RenderError::Resize(
            "cannot crop to a larger size than the source".to_owned(),
        ));
    }

    let left = ((source.width - width) / 2) as usize;
    let top = ((source.height - height) / 2) as usize;
    let src_stride = source.width as usize * 4;
    let dst_stride = width as usize * 4;

    let mut pixels = Vec::with_capacity(dst_stride * height as usize);
    for row in 0..height as usize {
        let start = (top + row) * src_stride + left * 4;
        pixels.extend_from_slice(&source.pixels[start..start + dst_stride]);
    }

    Rgba8::new(pixels, width, height)
}

/// Converts straight-alpha RGBA8 into a premultiplied `tiny_skia` pixmap.
///
/// # Errors
///
/// [`RenderError::MalformedBuffer`] if a pixmap of these dimensions cannot be
/// allocated.
///
/// # Panics
///
/// Does not panic. Clippy sees the `expect` inside the fallback path, which
/// is unreachable: `from_rgba` rejects only inputs where a colour channel
/// exceeds alpha, and premultiplying straight-alpha data cannot produce one.
pub fn to_pixmap(source: &Rgba8) -> Result<Pixmap, RenderError> {
    let mut pixmap =
        Pixmap::new(source.width, source.height).ok_or_else(|| RenderError::MalformedBuffer {
            expected: (source.width as usize) * (source.height as usize) * 4,
            found: 0,
        })?;

    for (target, chunk) in pixmap
        .pixels_mut()
        .iter_mut()
        .zip(source.pixels.as_chunks::<4>().0)
    {
        // from_rgba premultiplies. It returns None only for inputs where a
        // channel exceeds alpha, which straight-alpha data cannot produce.
        *target = PremultipliedColorU8::from_rgba(
            multiply(chunk[0], chunk[3]),
            multiply(chunk[1], chunk[3]),
            multiply(chunk[2], chunk[3]),
            chunk[3],
        )
        .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).expect("zero is valid"));
    }

    Ok(pixmap)
}

/// Converts a premultiplied pixmap back into straight-alpha RGBA8.
///
/// # Errors
///
/// [`RenderError::MalformedBuffer`] if the pixmap length is inconsistent.
pub fn from_pixmap(pixmap: &Pixmap) -> Result<Rgba8, RenderError> {
    let mut pixels = Vec::with_capacity(pixmap.pixels().len() * 4);
    for pixel in pixmap.pixels() {
        let alpha = pixel.alpha();
        pixels.extend_from_slice(&[
            unmultiply(pixel.red(), alpha),
            unmultiply(pixel.green(), alpha),
            unmultiply(pixel.blue(), alpha),
            alpha,
        ]);
    }

    Rgba8::new(pixels, pixmap.width(), pixmap.height())
}

/// Multiplies a colour channel by alpha, rounding.
///
/// The standard `(x * a + 127 + ((x * a + 127) >> 8)) >> 8` approximation of
/// `x * a / 255`. Both inputs are `u8`, so the product is at most 65 025 and
/// the result at most 255; the cast cannot truncate.
#[allow(clippy::cast_possible_truncation)]
fn multiply(channel: u8, alpha: u8) -> u8 {
    let product = u32::from(channel) * u32::from(alpha) + 127;
    ((product + (product >> 8)) >> 8) as u8
}

/// Divides a premultiplied channel by alpha.
///
/// Clamped to 255 before the cast, so truncation is excluded explicitly rather
/// than by argument.
#[allow(clippy::cast_possible_truncation)]
fn unmultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    let value = (u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
    value.min(255) as u8
}
