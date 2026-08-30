//! Object path layout.
//!
//! # Invariant
//!
//! Every path here is a pure function of its inputs and contains no
//! timestamp, no random component and no hostname. Two processes deriving a
//! path for the same content must agree, or the cache silently splits.
//!
//! Prefixes are separate top-level segments (`l2/`, `spec/`) rather than
//! suffixes on one namespace, so that a bucket lifecycle rule can address one
//! without being able to reach the other.
//!
//! Source artwork has no path here. Nothing fetched from TMDB is written to
//! storage — see `docs/adr/0007-do-not-persist-source-artwork.md`.

use crate::spec::CacheKey;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> CacheKey {
        CacheKey::from_hex(&"ab".repeat(32)).expect("valid hex")
    }

    #[test]
    fn tiers_occupy_distinct_prefixes() {
        // A lifecycle rule matches on prefix, so the two must not share one.
        assert!(l2_poster(key()).starts_with("l2/"));
        assert!(spec(key()).starts_with("spec/"));
    }

    #[test]
    fn paths_are_deterministic() {
        assert_eq!(l2_poster(key()), l2_poster(key()));
        assert_eq!(spec(key()), spec(key()));
    }

    #[test]
    fn a_poster_and_its_specification_are_separate_objects() {
        assert_ne!(l2_poster(key()), spec(key()));
    }

    #[test]
    fn distinct_keys_address_distinct_objects() {
        let other = CacheKey::from_hex(&"cd".repeat(32)).expect("valid hex");
        assert_ne!(l2_poster(key()), l2_poster(other));
        assert_ne!(spec(key()), spec(other));
    }

    #[test]
    fn stored_paths_have_no_traversal_potential() {
        // Every component is hex or a fixed literal, so a stored path cannot
        // escape its prefix however the request that produced it was spelled.
        for path in [l2_poster(key()), spec(key())] {
            assert!(!path.contains(".."), "{path} contains traversal");
            assert!(!path.starts_with('/'), "{path} is absolute");
            assert!(
                path.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_')),
                "{path} has an unexpected character"
            );
        }
    }
}
