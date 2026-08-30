//! TMDB source addressing.
//!
//! # Invariant
//!
//! A [`PosterPath`] can only be constructed from a string that matches the
//! grammar in [`PosterPath::parse`]. There is no other constructor, no
//! `From<String>`, and the inner field is private, so a value of this type is
//! proof that the path is safe to append to the configured CDN base.
//!
//! That proof is the entire SSRF control. The API never accepts a URL: it
//! accepts a path, and builds the URL server-side from a configured base. No
//! input can therefore produce a request to a host the operator did not
//! choose. This is a structural guarantee rather than a filter, which matters
//! because filters lose to DNS rebinding, IPv6-mapped addresses, redirect
//! chains and decimal IP encodings, and a constant host loses to none of them.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// Rejection reason for a malformed TMDB path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPosterPath {
    /// The path did not begin with `/`.
    #[error("path must begin with '/'")]
    MissingLeadingSlash,
    /// The stem was outside the accepted length range.
    #[error("path stem must be {min}-{max} characters, found {found}")]
    StemLength {
        /// Shortest accepted stem.
        min: usize,
        /// Longest accepted stem.
        max: usize,
        /// Length actually supplied.
        found: usize,
    },
    /// The stem contained a character outside `[A-Za-z0-9]`.
    #[error("path stem must be alphanumeric")]
    NonAlphanumericStem,
    /// The extension was missing or unrecognised.
    #[error("path must end in .jpg, .png or .webp")]
    UnsupportedExtension,
}

/// Which TMDB image family a path belongs to.
///
/// TMDB serves posters and backdrops from the same host under the same size
/// prefixes, but at different aspect ratios. The renderer needs to know which
/// it received in order to decide how to fit the artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 2:3 portrait artwork. The default, and the common case.
    #[default]
    Poster,
    /// 16:9 landscape artwork, cropped to fit.
    Backdrop,
}

impl SourceKind {
    /// Returns the stable byte used to represent this variant in a cache key.
    ///
    /// Written out rather than derived from the enum's discriminant: a derived
    /// value would silently change if a variant were ever inserted above
    /// another, and that would orphan every cache key without any visible
    /// cause. See [`crate::spec::key`].
    #[must_use]
    pub const fn key_tag(self) -> u8 {
        match self {
            Self::Poster => 0,
            Self::Backdrop => 1,
        }
    }
}

/// A validated TMDB image path, for example `/kqjL17yufvn9OVLyXYpvtyrFfak.jpg`.
///
/// See the module documentation for the invariant this type carries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PosterPath(String);

/// Shortest stem TMDB is known to issue, less a safety margin.
const STEM_MIN: usize = 20;
/// Longest stem TMDB is known to issue, plus a safety margin.
const STEM_MAX: usize = 60;

impl PosterPath {
    /// Validates a TMDB image path.
    ///
    /// The accepted grammar is `/[A-Za-z0-9]{20,60}\.(jpg|png|webp)`.
    ///
    /// Implemented by hand rather than with the `regex` crate. The grammar is
    /// three checks long, so a regex would add a substantial dependency and a
    /// compiled automaton to express something a reader can verify by eye —
    /// and the hand-written version can report *which* rule failed, which a
    /// single regex match cannot.
    ///
    /// The alphanumeric check is deliberately ASCII-only. `char::is_alphanumeric`
    /// would accept Cyrillic and fullwidth characters that look identical to
    /// Latin ones in a log line, so the check uses [`u8::is_ascii_alphanumeric`]
    /// over the bytes.
    ///
    /// # Arguments
    ///
    /// * `raw` — the candidate path, including its leading slash.
    ///
    /// # Errors
    ///
    /// Returns the specific [`InvalidPosterPath`] rule that the input broke.
    ///
    /// # Examples
    ///
    /// ```
    /// use poster_service::tmdb::PosterPath;
    ///
    /// let path = PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")?;
    /// assert_eq!(path.as_str(), "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg");
    ///
    /// // Traversal, absolute URLs and protocol-relative URLs are unrepresentable.
    /// assert!(PosterPath::parse("/../../etc/passwd").is_err());
    /// assert!(PosterPath::parse("https://evil.test/x.jpg").is_err());
    /// assert!(PosterPath::parse("//evil.test/x.jpg").is_err());
    /// # Ok::<(), poster_service::tmdb::InvalidPosterPath>(())
    /// ```
    pub fn parse(raw: &str) -> Result<Self, InvalidPosterPath> {
        let rest = raw
            .strip_prefix('/')
            .ok_or(InvalidPosterPath::MissingLeadingSlash)?;

        // rsplit_once rather than split_once: a stem cannot contain a dot under
        // this grammar, but splitting from the right means a path with several
        // dots fails the alphanumeric check with a useful message instead of
        // failing on an extension that was never the problem.
        let (stem, extension) = rest
            .rsplit_once('.')
            .ok_or(InvalidPosterPath::UnsupportedExtension)?;

        if !matches!(extension, "jpg" | "png" | "webp") {
            return Err(InvalidPosterPath::UnsupportedExtension);
        }

        let found = stem.len();
        if !(STEM_MIN..=STEM_MAX).contains(&found) {
            return Err(InvalidPosterPath::StemLength {
                min: STEM_MIN,
                max: STEM_MAX,
                found,
            });
        }

        if !stem.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(InvalidPosterPath::NonAlphanumericStem);
        }

        Ok(Self(raw.to_owned()))
    }

    /// Returns the validated path, leading slash included.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds the CDN URL for this path at the given size.
    ///
    /// # Arguments
    ///
    /// * `base` — the configured CDN base, for example `https://image.tmdb.org/t/p`.
    /// * `size` — the TMDB size segment.
    ///
    /// # Returns
    ///
    /// `{base}/{size}{path}`. Because `self` is a [`PosterPath`], the host is
    /// always the one in `base`.
    ///
    /// # Examples
    ///
    /// ```
    /// use poster_service::tmdb::{PosterPath, TmdbSize};
    ///
    /// let path = PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")?;
    /// assert_eq!(
    ///     path.cdn_url("https://image.tmdb.org/t/p", TmdbSize::W1280),
    ///     "https://image.tmdb.org/t/p/w1280/kqjL17yufvn9OVLyXYpvtyrFfak.jpg"
    /// );
    /// # Ok::<(), poster_service::tmdb::InvalidPosterPath>(())
    /// ```
    #[must_use]
    pub fn cdn_url(&self, base: &str, size: TmdbSize) -> String {
        format!("{}/{}{}", base.trim_end_matches('/'), size.as_str(), self.0)
    }
}

impl fmt::Display for PosterPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PosterPath {
    /// Deserialises through [`PosterPath::parse`].
    ///
    /// Written by hand rather than derived so that the invariant holds for
    /// every construction path. A derived implementation on a newtype would
    /// accept any string and quietly defeat the type's whole purpose.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A TMDB CDN size segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmdbSize {
    /// 780 px wide. Sufficient upstream resolution for a w1000 render.
    W780,
    /// 1280 px wide. Used for w2000 renders.
    W1280,
    /// Original upload. Avoided on the request path: TMDB originals are
    /// occasionally very large, and the byte cap would reject them after
    /// paying for the transfer.
    Original,
}

impl TmdbSize {
    /// Returns the path segment for this size.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::W780 => "w780",
            Self::W1280 => "w1280",
            Self::Original => "original",
        }
    }
}
