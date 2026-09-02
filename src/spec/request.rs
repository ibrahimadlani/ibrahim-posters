//! The wire shape of a poster request, and its syntactic validation.
//!
//! Types here are *unresolved*: a preset is named but not applied, overrides
//! are present but not merged, and nothing has been clamped. Nothing in this
//! module may be hashed — see [`crate::spec::key`] for why the raw request is
//! the wrong hash input.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization as _;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::spec::clamp;
use crate::tmdb::api::MediaKind;
use crate::tmdb::PosterPath;

/// Why a request was rejected before resolution.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// The named preset is not in the catalogue.
    #[error("unknown preset: {0}")]
    UnknownPreset(String),
    /// More badges than the top row can hold.
    #[error("at most {max} badges are allowed, found {found}")]
    TooManyBadges {
        /// Configured maximum.
        max: usize,
        /// Number supplied.
        found: usize,
    },
    /// A badge's text was longer than the layout can hold.
    #[error("badge {index} is {found} graphemes, at most {max} are allowed")]
    BadgeTextTooLong {
        /// Position of the offending badge in the row.
        index: usize,
        /// Configured maximum.
        max: usize,
        /// Length actually supplied.
        found: usize,
    },
    /// The title offers no poster this service can render.
    #[error("{0}")]
    NoArtwork(String),
    /// The chosen artwork is not offered by the requested title.
    #[error("{path} is not artwork this title offers")]
    ArtworkNotOffered {
        /// The path that was asked for.
        path: String,
    },
    /// A badge's text was empty, or became empty once control characters were
    /// removed.
    #[error("badge {index} has no renderable text")]
    BadgeTextEmpty {
        /// Position of the offending badge in the row.
        index: usize,
    },
    /// A caption's genre label was longer than the line can hold.
    #[error("caption genre is limited to {max} characters, found {found}")]
    CaptionGenreTooLong {
        /// Configured maximum.
        max: usize,
        /// Length supplied.
        found: usize,
    },
    /// Neither catalogue identifier was supplied.
    #[error("exactly one of tmdb_movie_id or tmdb_tv_id is required")]
    NoIdentifier,
    /// Both catalogue identifiers were supplied.
    #[error("tmdb_movie_id and tmdb_tv_id are mutually exclusive")]
    AmbiguousIdentifier,
}

/// Output resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputWidth {
    /// 1000x1500. The default, and the size the latency budget is stated for.
    #[default]
    W1000,
    /// 2000x3000. Four times the pixels; see `PLAN.md` section 14.5.
    W2000,
}

impl OutputWidth {
    /// Returns the output dimensions in pixels, width then height.
    ///
    /// The 2:3 ratio is fixed. Posters are a standardised shape, and letting
    /// the caller choose one would multiply the reference images the visual
    /// regression suite has to hold for no product benefit.
    #[must_use]
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::W1000 => (1000, 1500),
            Self::W2000 => (2000, 3000),
        }
    }

    /// Scale factor relative to w1000.
    ///
    /// Preset geometry is authored at w1000 and multiplied by this, so that a
    /// preset describes one layout rather than one layout per size.
    #[must_use]
    pub const fn scale(self) -> f32 {
        match self {
            Self::W1000 => 1.0,
            Self::W2000 => 2.0,
        }
    }

    /// Integer scale for pixel-valued fields at this width.
    ///
    /// Separate from [`OutputWidth::scale`] so that integer fields such as
    /// badge height scale without a float round trip, which at w2000 would
    /// turn an exact doubling into a value that depends on rounding mode.
    #[must_use]
    pub const fn pixel_scale(self) -> u32 {
        match self {
            Self::W1000 => 1,
            Self::W2000 => 2,
        }
    }

    /// Returns the stable byte representing this variant in a cache key.
    ///
    /// Written out rather than derived; see [`SourceKind::key_tag`].
    #[must_use]
    pub const fn key_tag(self) -> u8 {
        match self {
            Self::W1000 => 0,
            Self::W2000 => 1,
        }
    }
}

/// Visual treatment for a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgeStyle {
    /// Filled pill, light text.
    #[default]
    Solid,
    /// Transparent pill with a stroked border.
    Outline,
    /// Filled pill in the accent colour, for the one badge that matters most.
    Accent,
}

impl BadgeStyle {
    /// Returns the stable byte representing this variant in a cache key.
    #[must_use]
    pub const fn key_tag(self) -> u8 {
        match self {
            Self::Solid => 0,
            Self::Outline => 1,
            Self::Accent => 2,
        }
    }
}

/// A single badge in the top row.
///
/// Width is derived from the rendered text advance rather than supplied by the
/// caller. A caller-supplied width would let a request ask for a badge
/// narrower than its own text, producing clipped output behind an immutable
/// cache header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Badge {
    /// Badge text, NFC-normalised during validation.
    pub text: String,
    /// Visual treatment.
    #[serde(default)]
    pub style: BadgeStyle,
}

impl Badge {
    /// Normalises and checks this badge's text.
    ///
    /// Applies NFC normalisation, strips control characters, and measures the
    /// result in grapheme clusters.
    ///
    /// Normalisation happens before measurement and before hashing for one
    /// reason: `"é"` written precomposed and written as `e` plus a combining
    /// accent are the same badge to a reader, and without normalisation they
    /// would produce two cache keys and two identical renders.
    ///
    /// Control characters are removed rather than rejected. They are invisible,
    /// so a rejection would report a problem the caller cannot see in their own
    /// input; removing them is both safer for the SVG builder and kinder.
    ///
    /// # Arguments
    ///
    /// * `index` — position in the row, used only for error reporting.
    ///
    /// # Errors
    ///
    /// [`SpecError::BadgeTextEmpty`] if nothing renderable remains, and
    /// [`SpecError::BadgeTextTooLong`] beyond
    /// [`clamp::BADGE_TEXT_GRAPHEMES`] clusters.
    pub fn normalised(&self, index: usize) -> Result<Self, SpecError> {
        let text: String = self
            .text
            .nfc()
            // Retain the ordinary space: it is a control character by no
            // definition that matters here, and badge text is usually
            // several words.
            .filter(|c| !c.is_control())
            .collect();
        let text = text.trim().to_owned();

        if text.is_empty() {
            return Err(SpecError::BadgeTextEmpty { index });
        }

        let found = text.graphemes(true).count();
        if found > clamp::BADGE_TEXT_GRAPHEMES {
            return Err(SpecError::BadgeTextTooLong {
                index,
                max: clamp::BADGE_TEXT_GRAPHEMES,
                found,
            });
        }

        Ok(Self {
            text,
            style: self.style,
        })
    }
}

/// The genre and rating line beneath the logo.
///
/// Both halves are optional and either alone is a valid caption, because the
/// two come from different places: a genre is almost always known, a rating is
/// not meaningful until a title has been released.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caption {
    /// Genre label, NFC-normalised during validation.
    #[serde(default)]
    pub genre: Option<String>,
    /// Star rating out of ten, clamped and rendered to one decimal.
    #[serde(default)]
    pub rating: Option<f32>,
}

/// Separator between the genre and the rating.
const CAPTION_SEPARATOR: &str = " \u{b7} ";

impl Caption {
    /// Normalises and checks this caption.
    ///
    /// # Returns
    ///
    /// The normalised caption, or `None` if neither half survives — an empty
    /// caption is the same thing as no caption, and treating it as absent
    /// spares the renderer a case that would draw nothing.
    ///
    /// # Errors
    ///
    /// [`SpecError::CaptionGenreTooLong`] beyond
    /// [`clamp::CAPTION_GENRE_GRAPHEMES`] clusters.
    pub fn normalised(&self) -> Result<Option<Self>, SpecError> {
        let genre = match &self.genre {
            None => None,
            Some(raw) => {
                let text: String = raw.nfc().filter(|c| !c.is_control()).collect();
                let text = text.trim().to_owned();
                let found = text.graphemes(true).count();
                if found > clamp::CAPTION_GENRE_GRAPHEMES {
                    return Err(SpecError::CaptionGenreTooLong {
                        max: clamp::CAPTION_GENRE_GRAPHEMES,
                        found,
                    });
                }
                (!text.is_empty()).then_some(text)
            }
        };

        let rating = self
            .rating
            .map(|value| clamp::f32_into(value, clamp::RATING));

        if genre.is_none() && rating.is_none() {
            return Ok(None);
        }
        Ok(Some(Self { genre, rating }))
    }

    /// Returns the line as it is drawn.
    ///
    /// The star is U+2605, which the embedded face carries, rather than a
    /// hand-built path: a glyph is shaped and hinted alongside the digits
    /// beside it, and a path would have to be positioned and scaled by hand
    /// against a baseline the shaper owns.
    #[must_use]
    pub fn line(&self) -> String {
        let rating = self.rating.map(|value| format!("\u{2605} {value:.1}"));
        match (&self.genre, rating) {
            (Some(genre), Some(rating)) => format!("{genre}{CAPTION_SEPARATOR}{rating}"),
            (Some(genre), None) => genre.clone(),
            (None, Some(rating)) => rating,
            (None, None) => String::new(),
        }
    }
}

/// Per-request deviations from a preset.
///
/// Every field is optional, and `None` means "inherit from the preset" rather
/// than "zero". That distinction is why this type cannot be collapsed into
/// [`crate::spec::Preset`] and why the merge in
/// [`crate::spec::Preset::resolve`] is written out rather than derived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    /// Height of the blurred band as a fraction of poster height.
    pub blur_band_fraction: Option<f32>,
    /// Gaussian sigma at w1000, in pixels.
    pub blur_sigma: Option<f32>,
    /// Peak opacity of the darkening ramp.
    pub darken_strength: Option<f32>,
    /// Logo width as a fraction of poster width.
    pub logo_width_fraction: Option<f32>,
    /// Distance from the bottom edge to the logo, as a fraction of height.
    pub logo_bottom_fraction: Option<f32>,
    /// Badge row height in pixels at w1000.
    pub badge_height: Option<u32>,
}

/// Which poster to composite on.
///
/// Serialised as a string: `"auto"`, or a TMDB path. The two are
/// unambiguous because every TMDB path begins with `/`, so no escaping or
/// tagged representation is needed to tell a mode from a selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum PosterChoice {
    /// Let the service pick, using the ranking in [`crate::tmdb::api`].
    #[default]
    Auto,
    /// Use this specific poster, which must be one the title offers.
    Explicit(PosterPath),
}

/// Which logo to place, if any.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum LogoChoice {
    /// Let the service pick, or render without one if the title has none.
    #[default]
    Auto,
    /// Render no logo even if the title has one.
    Omit,
    /// Use this specific logo, which must be one the title offers.
    Explicit(PosterPath),
}

impl<'de> Deserialize<'de> for PosterChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "auto" => Ok(Self::Auto),
            path => PosterPath::parse(path)
                .map(Self::Explicit)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl<'de> Deserialize<'de> for LogoChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "auto" => Ok(Self::Auto),
            "none" => Ok(Self::Omit),
            path => PosterPath::parse(path)
                .map(Self::Explicit)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// A poster generation request as it arrives on the wire.
///
/// Artwork is named by TMDB catalogue identifier, not by path. The service
/// resolves the identifier to a poster and a logo itself, so a caller needs to
/// know only which film or series they want.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PosterRequest {
    /// TMDB film identifier. Mutually exclusive with `tmdb_tv_id`.
    #[serde(default)]
    pub tmdb_movie_id: Option<u32>,
    /// TMDB series identifier. Mutually exclusive with `tmdb_movie_id`.
    #[serde(default)]
    pub tmdb_tv_id: Option<u32>,
    /// Preferred language for the title logo, as an ISO 639-1 code.
    ///
    /// Logos are language-specific on TMDB. A title with no logo in the
    /// requested language falls back to a language-neutral one, then to any
    /// other, rather than rendering without one.
    #[serde(default = "default_language")]
    pub language: String,
    /// Which poster to composite on.
    ///
    /// Defaults to the service's ranking. Send a path from
    /// `GET /v1/artwork/{kind}/{id}` to choose a different one.
    #[serde(default)]
    pub poster: PosterChoice,
    /// Which logo to place.
    ///
    /// Defaults to the service's ranking, `"none"` to render without one, or
    /// a path from the same catalogue endpoint.
    #[serde(default)]
    pub logo: LogoChoice,
    /// Named layout preset.
    #[serde(default = "default_preset_name")]
    pub preset: String,
    /// Badges rendered along the top edge, left to right.
    #[serde(default)]
    pub badges: Vec<Badge>,
    /// Genre and rating line beneath the logo. Omitted by default.
    #[serde(default)]
    pub caption: Option<Caption>,
    /// Output resolution.
    #[serde(default)]
    pub width: OutputWidth,
    /// Per-request deviations from the preset.
    #[serde(default)]
    pub overrides: Overrides,
}

/// Language used when a request does not choose one.
fn default_language() -> String {
    "en".to_owned()
}

impl PosterRequest {
    /// Returns which catalogue entry this request names.
    ///
    /// # Errors
    ///
    /// [`SpecError::NoIdentifier`] if neither identifier is present, and
    /// [`SpecError::AmbiguousIdentifier`] if both are. Requiring exactly one
    /// rather than preferring a winner means a caller who sends both learns
    /// they were confused instead of silently getting whichever the
    /// implementation happened to check first.
    pub fn target(&self) -> Result<(MediaKind, u32), SpecError> {
        match (self.tmdb_movie_id, self.tmdb_tv_id) {
            (Some(id), None) => Ok((MediaKind::Movie, id)),
            (None, Some(id)) => Ok((MediaKind::Tv, id)),
            (None, None) => Err(SpecError::NoIdentifier),
            (Some(_), Some(_)) => Err(SpecError::AmbiguousIdentifier),
        }
    }

    /// Returns the requested logo language, normalised.
    ///
    /// Lowercased and truncated to the two-letter ISO 639-1 form TMDB uses, so
    /// `EN`, `en` and `en-GB` all address the same artwork rather than
    /// producing three cache keys for one poster.
    #[must_use]
    pub fn normalised_language(&self) -> String {
        self.language
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .take(2)
            .flat_map(char::to_lowercase)
            .collect()
    }
}

/// Name of the preset applied when a request does not choose one.
fn default_preset_name() -> String {
    "standard".to_owned()
}

impl PosterRequest {
    /// Validates and normalises the badge row.
    ///
    /// # Returns
    ///
    /// The badges with their text NFC-normalised and trimmed.
    ///
    /// # Errors
    ///
    /// [`SpecError::TooManyBadges`], or the first per-badge failure from
    /// [`Badge::normalised`].
    pub fn normalised_badges(&self) -> Result<Vec<Badge>, SpecError> {
        if self.badges.len() > clamp::MAX_BADGES {
            return Err(SpecError::TooManyBadges {
                max: clamp::MAX_BADGES,
                found: self.badges.len(),
            });
        }

        self.badges
            .iter()
            .enumerate()
            .map(|(index, badge)| badge.normalised(index))
            .collect()
    }

    /// Normalises the caption, if there is one.
    ///
    /// # Errors
    ///
    /// See [`Caption::normalised`].
    pub fn normalised_caption(&self) -> Result<Option<Caption>, SpecError> {
        self.caption.as_ref().map_or(Ok(None), Caption::normalised)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(text: &str) -> Badge {
        Badge {
            text: text.to_owned(),
            style: BadgeStyle::Solid,
        }
    }

    #[test]
    fn nfc_normalisation_collapses_equivalent_spellings() {
        // "é" precomposed (U+00E9) against "e" plus combining acute (U+0301).
        let precomposed = badge("Andr\u{e9}").normalised(0).expect("valid");
        let combining = badge("Andre\u{301}").normalised(0).expect("valid");

        assert_eq!(
            precomposed, combining,
            "equivalent spellings must converge, or they produce two cache keys"
        );
    }

    #[test]
    fn control_characters_are_stripped_not_rejected() {
        let badge = badge("Osc\u{7}ar\u{0}").normalised(0).expect("valid");
        assert_eq!(badge.text, "Oscar");
    }

    #[test]
    fn spaces_survive_control_character_stripping() {
        let badge = badge("Oscar Nominee").normalised(0).expect("valid");
        assert_eq!(badge.text, "Oscar Nominee");
    }

    #[test]
    fn text_that_is_only_control_characters_is_rejected() {
        assert_eq!(
            badge("\u{7}\u{0}").normalised(3),
            Err(SpecError::BadgeTextEmpty { index: 3 })
        );
    }

    #[test]
    fn the_limit_counts_graphemes_not_chars() {
        // A family emoji is one grapheme cluster and five chars, so eight of
        // them are 40 chars but only 8 clusters. Counting chars would reject
        // text that renders as eight glyphs and fits the layout comfortably.
        let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".repeat(8);
        assert_eq!(text.chars().count(), 40);
        assert!(text.chars().count() > clamp::BADGE_TEXT_GRAPHEMES);

        let normalised = badge(&text)
            .normalised(0)
            .expect("8 clusters is within the limit");
        assert_eq!(
            normalised.text.graphemes(true).count(),
            8,
            "the zero-width joiners must survive, or the family splits into six people"
        );
    }

    #[test]
    fn overlong_text_is_rejected_with_its_measurement() {
        let text = "a".repeat(clamp::BADGE_TEXT_GRAPHEMES + 1);
        assert_eq!(
            badge(&text).normalised(1),
            Err(SpecError::BadgeTextTooLong {
                index: 1,
                max: clamp::BADGE_TEXT_GRAPHEMES,
                found: clamp::BADGE_TEXT_GRAPHEMES + 1,
            })
        );
    }

    #[test]
    fn output_widths_keep_the_two_to_three_ratio() {
        for width in [OutputWidth::W1000, OutputWidth::W2000] {
            let (w, h) = width.dimensions();
            assert_eq!(w * 3, h * 2, "{width:?} is not 2:3");
        }
    }
}
