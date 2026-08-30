//! Content-addressed cache keys.
//!
//! # Why this module is written by hand
//!
//! Rendered posters are served with `Cache-Control: public, max-age=31536000,
//! immutable`, and there is no invalidation path. That header is only honest
//! if the key changes whenever the bytes would, so the derivation has to be
//! deterministic across processes, machines and dependency upgrades.
//!
//! `serde_json` is deliberately not involved. It makes no stable guarantee
//! about map key ordering, and float formatting has changed across versions.
//! Either change would silently reassign every key in the cache — presenting
//! as a total cache miss and a cost spike with no deploy to blame — so the
//! encoding is written out explicitly instead.
//!
//! # The encoding
//!
//! Fields are walked in [`ResolvedSpec`] declaration order. Every
//! variable-length field is length-prefixed, and every enum contributes an
//! explicitly written tag byte rather than a derived discriminant.
//!
//! The length prefixes are what make the encoding *injective*, which is the
//! property the whole scheme rests on. Without them, badge texts `["ab", "c"]`
//! and `["a", "bc"]` concatenate to the same bytes and two visually different
//! posters collide onto one cache entry — a correctness failure, not a
//! performance one.

use blake3::Hasher;

use crate::spec::resolved::ResolvedSpec;
use crate::RENDER_VERSION;

/// A 32-byte content-addressed cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey([u8; 32]);

/// Domain separator, mixed in first.
///
/// Guards against a future hash over some other type -- an L1 source key, say
/// -- producing the same digest as a spec key for a coincidentally identical
/// byte string.
const DOMAIN: &[u8] = b"poster-service/spec/v1";

impl CacheKey {
    /// Derives the key for a resolved specification.
    ///
    /// # Arguments
    ///
    /// * `spec` — a resolved, clamped specification.
    ///
    /// # Returns
    ///
    /// The blake3 digest of [`canonical_encoding`] mixed with
    /// [`crate::RENDER_VERSION`].
    #[must_use]
    pub fn of(spec: &ResolvedSpec) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(DOMAIN);
        hasher.update(&RENDER_VERSION.to_le_bytes());
        hasher.update(&canonical_encoding(spec));
        Self(*hasher.finalize().as_bytes())
    }

    /// Renders the key as 64 lowercase hexadecimal characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use std::fmt::Write as _;
        self.0.iter().fold(String::with_capacity(64), |mut out, b| {
            // Writing to a String is infallible; the Result exists only to
            // satisfy the fmt::Write signature.
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    /// Parses a key from its hexadecimal form.
    ///
    /// # Arguments
    ///
    /// * `hex` — exactly 64 hexadecimal characters.
    ///
    /// # Returns
    ///
    /// The key, or `None` if the input is not 64 valid hex characters. Used by
    /// the `GET` handler, where the key arrives from a URL and is therefore
    /// untrusted.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let pair = hex.get(index * 2..index * 2 + 2)?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(bytes))
    }

    /// Returns the raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Encodes a specification into its canonical byte string.
///
/// Public because it is the thing the injectivity property test asserts
/// against. That test is the honest form of "distinct specifications must not
/// collide": no test can establish blake3's collision resistance, but
/// injectivity of *this* function is falsifiable, and it catches the failure
/// that actually occurs in practice — a field left out of the encoding, or a
/// missing length prefix that lets two inputs share a representation.
///
/// # Arguments
///
/// * `spec` — the specification to encode.
///
/// # Returns
///
/// A byte string that is equal for equal specifications and distinct for
/// distinct ones.
///
/// # Examples
///
/// ```
/// use poster_service::spec::{key, preset, request::PosterRequest};
///
/// let json = r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#;
/// let spec = preset::resolve(&serde_json::from_str::<PosterRequest>(json)?)?;
///
/// assert_eq!(key::canonical_encoding(&spec), key::canonical_encoding(&spec));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use]
pub fn canonical_encoding(spec: &ResolvedSpec) -> Vec<u8> {
    // Building a buffer rather than feeding the hasher directly costs one small
    // allocation on a path that is already doing a storage write, and buys a
    // directly testable injectivity property. Worth it.
    let mut out = Vec::with_capacity(256);

    push_str(&mut out, spec.source.as_str());
    out.push(spec.source_kind.key_tag());

    // The tag distinguishes absent from present-and-empty. A bare
    // length-prefixed string would encode None and Some("") identically.
    match &spec.logo {
        None => out.push(0),
        Some(logo) => {
            out.push(1);
            push_str(&mut out, logo.as_str());
        }
    }

    push_len(&mut out, spec.badges.len());
    for badge in &spec.badges {
        push_str(&mut out, &badge.text);
        out.push(badge.style.key_tag());
    }

    out.push(spec.width.key_tag());
    push_f32(&mut out, spec.blur_band_fraction);
    push_f32(&mut out, spec.blur_sigma);
    push_f32(&mut out, spec.darken_strength);
    push_f32(&mut out, spec.logo_width_fraction);
    push_f32(&mut out, spec.logo_bottom_fraction);
    out.extend_from_slice(&spec.badge_height.to_le_bytes());

    out
}

/// Appends a length-prefixed UTF-8 string.
fn push_str(out: &mut Vec<u8>, value: &str) {
    push_len(out, value.len());
    out.extend_from_slice(value.as_bytes());
}

/// Appends a length as a fixed-width little-endian `u64`.
///
/// Fixed width rather than a varint so that the prefix itself cannot be
/// confused with the payload that follows it.
fn push_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u64).to_le_bytes());
}

/// Appends a float by its bit pattern, with negative zero canonicalised.
///
/// `-0.0` and `0.0` are equal under `PartialEq` but have different bit
/// patterns, so hashing the raw bits would give two keys for two
/// specifications that compare equal — breaking the property that equal specs
/// produce equal keys. Clamping cannot itself produce `-0.0`, but an override
/// of `-0.0` passes through a range starting at `0.0` unchanged, so the case
/// is reachable from the API rather than theoretical.
///
/// NaN cannot reach here: `clamp::f32_into` maps it to a range start, and JSON
/// has no NaN literal.
fn push_f32(out: &mut Vec<u8>, value: f32) {
    let canonical = if value == 0.0 { 0.0 } else { value };
    out.extend_from_slice(&canonical.to_bits().to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::preset;
    use crate::spec::request::PosterRequest;

    fn spec_from(json: &str) -> ResolvedSpec {
        let request: PosterRequest = serde_json::from_str(json).expect("valid request");
        preset::resolve(&request).expect("resolves")
    }

    fn base() -> ResolvedSpec {
        spec_from(r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#)
    }

    #[test]
    fn the_key_is_deterministic() {
        assert_eq!(base().cache_key(), base().cache_key());
    }

    #[test]
    fn hex_round_trips() {
        let key = base().cache_key();
        assert_eq!(CacheKey::from_hex(&key.to_hex()), Some(key));
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert_eq!(CacheKey::from_hex(""), None);
        assert_eq!(CacheKey::from_hex(&"z".repeat(64)), None);
        assert_eq!(CacheKey::from_hex(&"a".repeat(63)), None);
        assert_eq!(CacheKey::from_hex(&"a".repeat(65)), None);
    }

    #[test]
    fn absent_logo_differs_from_present_logo() {
        let without = base();
        let with = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png" }"#,
        );
        assert_ne!(without.cache_key(), with.cache_key());
    }

    #[test]
    fn badge_boundaries_are_unambiguous() {
        // The case length prefixes exist for: without them these two encode
        // identically and collide onto one cache entry.
        let ab_c = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "badges": [{ "text": "ab" }, { "text": "c" }] }"#,
        );
        let a_bc = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "badges": [{ "text": "a" }, { "text": "bc" }] }"#,
        );
        assert_ne!(canonical_encoding(&ab_c), canonical_encoding(&a_bc));
        assert_ne!(ab_c.cache_key(), a_bc.cache_key());
    }

    #[test]
    fn badge_order_is_significant() {
        let forward = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "badges": [{ "text": "a" }, { "text": "b" }] }"#,
        );
        let reverse = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "badges": [{ "text": "b" }, { "text": "a" }] }"#,
        );
        assert_ne!(forward.cache_key(), reverse.cache_key());
    }

    #[test]
    fn equivalent_requests_converge_on_one_key() {
        // An override that restates the preset default, and one that clamps to
        // it, must not produce extra cache entries.
        let implicit = base();
        let explicit = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "preset": "standard",
                 "overrides": { "blur_sigma": 24.0 } }"#,
        );
        let clamped = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "overrides": { "darken_strength": 99.0 } }"#,
        );
        let at_cap = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "overrides": { "darken_strength": 0.95 } }"#,
        );

        assert_eq!(implicit.cache_key(), explicit.cache_key());
        assert_eq!(clamped.cache_key(), at_cap.cache_key());
    }

    #[test]
    fn negative_zero_does_not_fork_the_key() {
        let positive = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "overrides": { "blur_sigma": 0.0 } }"#,
        );
        let negative = spec_from(
            r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                 "overrides": { "blur_sigma": -0.0 } }"#,
        );
        assert_eq!(positive, negative);
        assert_eq!(positive.cache_key(), negative.cache_key());
    }

    #[test]
    fn width_changes_the_key() {
        let small = base();
        let large =
            spec_from(r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "width": "w2000" }"#);
        assert_ne!(small.cache_key(), large.cache_key());
    }

    #[test]
    fn the_encoding_starts_with_the_source_path() {
        let spec = base();
        let encoded = canonical_encoding(&spec);
        let len = spec.source.as_str().len();
        assert_eq!(&encoded[..8], &(len as u64).to_le_bytes());
        assert_eq!(&encoded[8..8 + len], spec.source.as_str().as_bytes());
    }
}
