//! The canonical, fully resolved render specification.

use serde::Serialize;

use crate::spec::key::CacheKey;
use crate::spec::request::{Badge, OutputWidth};
use crate::tmdb::{PosterPath, SourceKind};

/// A fully resolved, clamped, canonical render specification.
///
/// This is the only input the renderer accepts, and the only thing that is
/// ever hashed.
///
/// # Invariants
///
/// - Produced only by [`crate::spec::preset::resolve`]; there is no public
///   constructor and no way to build one that skipped clamping.
/// - Every numeric field is within its range in [`crate::spec::clamp`].
/// - Badge text is NFC-normalised, trimmed, and free of control characters.
/// - **Field order is hashing order.** [`crate::spec::key`] walks these fields
///   in declaration order, so reordering the struct changes every cache key
///   the service has ever issued. That is survivable — it behaves exactly like
///   a `RENDER_VERSION` bump — but it is never what someone tidying a struct
///   intends, which is why it is stated here.
/// - Two requests that differ only in ways the renderer ignores resolve to
///   equal values. This is what makes the target cache hit rate reachable, and
///   it is asserted by property test rather than assumed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedSpec {
    /// Background artwork.
    pub source: PosterPath,
    /// Which TMDB image family the background belongs to.
    pub source_kind: SourceKind,
    /// Optional title logo.
    pub logo: Option<PosterPath>,
    /// Badges along the top edge, left to right.
    pub badges: Vec<Badge>,
    /// Output resolution.
    pub width: OutputWidth,
    /// Height of the blurred band as a fraction of poster height.
    pub blur_band_fraction: f32,
    /// Gaussian sigma in pixels, already scaled to the output width.
    pub blur_sigma: f32,
    /// Peak opacity of the darkening ramp.
    pub darken_strength: f32,
    /// Logo width as a fraction of poster width.
    pub logo_width_fraction: f32,
    /// Distance from the bottom edge to the logo, as a fraction of height.
    pub logo_bottom_fraction: f32,
    /// Badge row height in pixels, already scaled to the output width.
    pub badge_height: u32,
}

impl ResolvedSpec {
    /// Computes the content-addressed cache key for this specification.
    ///
    /// See [`crate::spec::key`] for the encoding and the reasoning behind it.
    ///
    /// # Returns
    ///
    /// A [`CacheKey`] over the canonical encoding of every field, mixed with
    /// [`crate::RENDER_VERSION`].
    ///
    /// # Examples
    ///
    /// ```
    /// use poster_service::spec::{preset, request::PosterRequest};
    ///
    /// let json = r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#;
    /// let request: PosterRequest = serde_json::from_str(json)?;
    /// let spec = preset::resolve(&request)?;
    ///
    /// // The key is stable: the same specification always yields the same key.
    /// assert_eq!(spec.cache_key(), spec.cache_key());
    /// assert_eq!(spec.cache_key().to_hex().len(), 64);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn cache_key(&self) -> CacheKey {
        CacheKey::of(self)
    }

    /// Returns the output dimensions in pixels, width then height.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        self.width.dimensions()
    }
}

#[cfg(test)]
mod tests {
    use crate::spec::{preset, request::PosterRequest};

    fn resolve(json: &str) -> super::ResolvedSpec {
        let request: PosterRequest = serde_json::from_str(json).expect("valid");
        preset::resolve(&request).expect("resolves")
    }

    #[test]
    fn dimensions_follow_the_requested_width() {
        let small = resolve(r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#);
        let large =
            resolve(r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "width": "w2000" }"#);

        assert_eq!(small.dimensions(), (1000, 1500));
        assert_eq!(large.dimensions(), (2000, 3000));
    }

    #[test]
    fn the_blur_band_is_a_whole_number_of_pixels_at_both_widths() {
        // The band height is a fraction of poster height, and the renderer
        // will slice rows by it. A fraction that lands mid-pixel is fine --
        // it truncates -- but a band of zero rows would make the blur stage a
        // silent no-op, which is worth catching here rather than visually.
        for json in [
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#,
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "width": "w2000" }"#,
        ] {
            let spec = resolve(json);
            let (_, height) = spec.dimensions();
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let band = (height as f32 * spec.blur_band_fraction) as u32;
            assert!(band > 0, "blur band rounded to zero rows");
            assert!(band <= height);
        }
    }
}
