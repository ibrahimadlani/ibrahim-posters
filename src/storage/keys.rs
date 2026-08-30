//! Object path layout.
//!
//! # Invariant
//!
//! Every path here is a pure function of its inputs and contains no
//! timestamp, no random component and no hostname. Two processes deriving a
//! path for the same content must agree, or the cache silently splits.
//!
//! Prefixes are separate top-level segments (`l1/`, `l2/`, `spec/`) rather
//! than suffixes on one namespace, because bucket lifecycle rules match on
//! prefix. The 30-day expiry on L1 is expressed as a rule on `l1/`, and it
//! must not be expressible in a way that could ever match `l2/`, whose
//! contents are advertised as immutable for a year.

use crate::spec::CacheKey;
use crate::spec::OutputWidth;
use crate::tmdb::PosterPath;

/// Path of a resized source image in the L1 tier.
///
/// L1 is keyed by the *source* and the target width, not by the poster spec:
/// every poster built from the same artwork at the same size shares one
/// resized intermediate, which is the entire reason the tier exists.
///
/// # Arguments
///
/// * `source` — the validated TMDB path.
/// * `width` — the output width the source was resized for.
///
/// # Returns
///
/// A path of the form `l1/{blake3-of-source}/{width}.webp`.
///
/// # Examples
///
/// ```
/// use poster_service::spec::OutputWidth;
/// use poster_service::storage::keys;
/// use poster_service::tmdb::PosterPath;
///
/// let source = PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")?;
/// let path = keys::l1_source(&source, OutputWidth::W1000);
///
/// assert!(path.starts_with("l1/"));
/// assert!(path.ends_with("/w1000.webp"));
/// # Ok::<(), poster_service::tmdb::InvalidPosterPath>(())
/// ```
#[must_use]
pub fn l1_source(source: &PosterPath, width: OutputWidth) -> String {
    // The source path is hashed rather than embedded. It is already
    // constrained to alphanumerics so it would be safe as a path segment, but
    // hashing keeps every stored key one fixed length, which makes listings
    // and lifecycle rules uniform.
    let digest = blake3::hash(source.as_str().as_bytes()).to_hex();
    format!("l1/{digest}/{}.webp", width_segment(width))
}

/// Path of a rendered poster in the L2 tier.
///
/// # Arguments
///
/// * `key` — the content-addressed key of the resolved specification.
///
/// # Returns
///
/// A path of the form `l2/{key}.webp`.
#[must_use]
pub fn l2_poster(key: CacheKey) -> String {
    format!("l2/{}.webp", key.to_hex())
}

/// Path of a persisted specification.
///
/// # Arguments
///
/// * `key` — the content-addressed key of the specification.
///
/// # Returns
///
/// A path of the form `spec/{key}.json`.
#[must_use]
pub fn spec(key: CacheKey) -> String {
    format!("spec/{}.json", key.to_hex())
}

/// Returns the path segment naming an output width.
///
/// Written out rather than derived from `Debug`, because a `Debug`
/// representation is not a stable contract and a change to it would orphan
/// the L1 tier.
const fn width_segment(width: OutputWidth) -> &'static str {
    match width {
        OutputWidth::W1000 => "w1000",
        OutputWidth::W2000 => "w2000",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> PosterPath {
        PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg").expect("valid")
    }

    fn key() -> CacheKey {
        CacheKey::from_hex(&"ab".repeat(32)).expect("valid hex")
    }

    #[test]
    fn tiers_occupy_distinct_prefixes() {
        // Lifecycle rules match on prefix: the 30-day expiry on l1/ must not
        // be expressible in a way that could reach l2/.
        assert!(l1_source(&source(), OutputWidth::W1000).starts_with("l1/"));
        assert!(l2_poster(key()).starts_with("l2/"));
        assert!(spec(key()).starts_with("spec/"));
    }

    #[test]
    fn paths_are_deterministic() {
        assert_eq!(
            l1_source(&source(), OutputWidth::W1000),
            l1_source(&source(), OutputWidth::W1000)
        );
        assert_eq!(l2_poster(key()), l2_poster(key()));
    }

    #[test]
    fn l1_separates_widths() {
        // One resized intermediate per width; sharing them would serve a
        // w1000 source into a w2000 render.
        assert_ne!(
            l1_source(&source(), OutputWidth::W1000),
            l1_source(&source(), OutputWidth::W2000)
        );
    }

    #[test]
    fn l1_separates_sources() {
        let other = PosterPath::parse("/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg").expect("valid");
        assert_ne!(
            l1_source(&source(), OutputWidth::W1000),
            l1_source(&other, OutputWidth::W1000)
        );
    }

    #[test]
    fn stored_paths_have_no_traversal_potential() {
        // Every component is hex or a fixed literal, so a stored path can
        // never escape its prefix however the source was spelled.
        for path in [
            l1_source(&source(), OutputWidth::W1000),
            l2_poster(key()),
            spec(key()),
        ] {
            assert!(!path.contains(".."), "{path} contains traversal");
            assert!(!path.starts_with('/'), "{path} is absolute");
            assert!(
                path.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_')),
                "{path} has an unexpected character"
            );
        }
    }

    #[test]
    fn width_segments_are_stable() {
        // These appear in stored paths; a change orphans the L1 tier.
        assert!(l1_source(&source(), OutputWidth::W1000).ends_with("w1000.webp"));
        assert!(l1_source(&source(), OutputWidth::W2000).ends_with("w2000.webp"));
    }
}
