//! The canonical, fully resolved render specification.

use serde::{Deserialize, Serialize};

use crate::spec::clamp;

use crate::spec::key::CacheKey;
use crate::spec::request::{Badge, OutputWidth};
use crate::tmdb::PosterPath;

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
///
/// # Deserialisation
///
/// `Deserialize` is derived so that persisted specifications can be read back,
/// but deriving it means a value can be constructed without passing through
/// [`crate::spec::preset::resolve`], which is where the clamping happens.
/// [`ResolvedSpec::validate`] closes that gap and every read from storage
/// calls it. The case is not hypothetical: a specification written by a build
/// with wider clamp ranges would deserialise cleanly and hand the renderer
/// geometry the current build considers out of bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedSpec {
    /// Background artwork, as resolved from a TMDB catalogue identifier.
    pub source: PosterPath,
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
    /// use poster_service::tmdb::api::{ArtworkOption, Catalogue, MediaKind};
    /// use poster_service::tmdb::PosterPath;
    ///
    /// let request: PosterRequest = serde_json::from_str(r#"{ "tmdb_movie_id": 27205 }"#)?;
    ///
    /// // Standing in for what a TMDB lookup would return.
    /// let catalogue = Catalogue {
    ///     kind: MediaKind::Movie,
    ///     id: 27205,
    ///     posters: vec![ArtworkOption {
    ///         path: PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")?,
    ///         language: Some("en".to_owned()),
    ///         vote_average: 8.0,
    ///         vote_count: 100,
    ///         width: 2000,
    ///         height: 3000,
    ///     }],
    ///     logos: Vec::new(),
    /// };
    /// let spec = preset::resolve(&request, &catalogue)?;
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

    /// Checks every field against its clamp range.
    ///
    /// Called on every specification read back from storage. A value produced
    /// by [`crate::spec::preset::resolve`] always passes; one that does not
    /// was deserialised from bytes this build did not write, and rendering it
    /// would produce geometry outside the range the renderer was tested for.
    ///
    /// Pixel-valued fields are compared against ranges scaled to the output
    /// width, matching how `resolve` produced them.
    ///
    /// # Errors
    ///
    /// [`SpecViolation`] naming the first field found out of range.
    pub fn validate(&self) -> Result<(), SpecViolation> {
        let scale = self.width.scale();
        let pixel_scale = self.width.pixel_scale();

        let checks: [(&'static str, bool); 6] = [
            (
                "blur_band_fraction",
                clamp::BLUR_BAND_FRACTION.contains(&self.blur_band_fraction),
            ),
            (
                "blur_sigma",
                self.blur_sigma >= *clamp::BLUR_SIGMA.start() * scale
                    && self.blur_sigma <= *clamp::BLUR_SIGMA.end() * scale,
            ),
            (
                "darken_strength",
                clamp::DARKEN_STRENGTH.contains(&self.darken_strength),
            ),
            (
                "logo_width_fraction",
                clamp::LOGO_WIDTH_FRACTION.contains(&self.logo_width_fraction),
            ),
            (
                "logo_bottom_fraction",
                clamp::LOGO_BOTTOM_FRACTION.contains(&self.logo_bottom_fraction),
            ),
            (
                "badge_height",
                self.badge_height >= *clamp::BADGE_HEIGHT.start() * pixel_scale
                    && self.badge_height <= *clamp::BADGE_HEIGHT.end() * pixel_scale,
            ),
        ];

        for (field, ok) in checks {
            if !ok {
                return Err(SpecViolation { field });
            }
        }

        if self.badges.len() > clamp::MAX_BADGES {
            return Err(SpecViolation { field: "badges" });
        }

        Ok(())
    }
}

/// A stored specification carried a field outside its permitted range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("field `{field}` is outside its permitted range")]
pub struct SpecViolation {
    /// Name of the offending field.
    pub field: &'static str,
}

#[cfg(test)]
mod tests {
    use crate::spec::{preset, request::PosterRequest};

    fn resolve(json: &str) -> super::ResolvedSpec {
        let request: PosterRequest = serde_json::from_str(json).expect("valid");
        let catalogue = crate::tmdb::api::Catalogue {
            kind: crate::tmdb::api::MediaKind::Movie,
            id: 27205,
            posters: vec![crate::tmdb::api::ArtworkOption {
                path: crate::tmdb::PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")
                    .expect("valid"),
                language: Some("en".to_owned()),
                vote_average: 5.0,
                vote_count: 10,
                width: 2000,
                height: 3000,
            }],
            logos: Vec::new(),
        };
        preset::resolve(&request, &catalogue).expect("resolves")
    }

    #[test]
    fn dimensions_follow_the_requested_width() {
        let small = resolve(r#"{ "tmdb_movie_id": 27205 }"#);
        let large = resolve(r#"{ "tmdb_movie_id": 27205, "width": "w2000" }"#);

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
            r#"{ "tmdb_movie_id": 27205 }"#,
            r#"{ "tmdb_movie_id": 27205, "width": "w2000" }"#,
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
