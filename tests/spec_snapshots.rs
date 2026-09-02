//! Snapshot tests for the resolved specification and the preset catalogue.
//!
//! These are the API contract. A snapshot diff is the review prompt that a
//! contract change deserves — and because the cache key is derived from these
//! exact values, a diff here is also notice that every key derived from the
//! affected preset has changed.

mod support;

use insta::{assert_json_snapshot, assert_snapshot};
use poster_service::spec::{preset, request::PosterRequest};

fn resolve(json: &str) -> poster_service::spec::ResolvedSpec {
    let request: PosterRequest = serde_json::from_str(json).expect("valid request");
    preset::resolve(&request, &support::catalogue()).expect("resolves")
}

#[test]
fn minimal_request_resolves_to_standard_preset() {
    let spec = resolve(r#"{ "tmdb_movie_id": 27205 }"#);
    assert_json_snapshot!(spec);
}

#[test]
fn full_request_with_overrides() {
    let spec = resolve(
        r##"{
            "tmdb_movie_id": 27205,
            "preset": "cinematic",
            "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png",
            "badges": [{ "text": "#17 IMDb", "style": "accent" }],
            "caption": { "genre": "Action", "rating": 6.5 },
            "width": "w2000",
            "overrides": { "blur_sigma": 30.0, "darken_strength": 0.8 }
        }"##,
    );
    assert_json_snapshot!(spec);
}

#[test]
fn out_of_range_overrides_are_clamped() {
    let spec = resolve(
        r#"{
            "tmdb_movie_id": 27205,
            "overrides": {
                "blur_band_fraction": 99.0,
                "blur_sigma": -50.0,
                "darken_strength": 4.0,
                "logo_width_fraction": 0.0,
                "logo_bottom_fraction": 100.0,
                "badge_height": 100000
            }
        }"#,
    );
    assert_json_snapshot!(spec);
}

#[test]
fn preset_catalogue() {
    let entries: Vec<_> = preset::catalogue_entries()
        .into_iter()
        .map(|(name, preset)| (name.to_owned(), preset))
        .collect();
    assert_json_snapshot!(entries);
}

/// Pins the cache keys of a fixed set of specifications.
///
/// This is the tripwire for an accidental change to the key derivation. Any
/// edit to `canonical_encoding`, to the field order of `ResolvedSpec`, to a
/// preset value, or to `RENDER_VERSION` moves these hex strings — and every
/// poster already cached under the old ones becomes unreachable.
///
/// A diff here is not necessarily a bug. It is a question: was the cache
/// meant to be invalidated? If yes, accept the snapshot. If no, something
/// changed that should not have.
#[test]
fn cache_keys_are_pinned() {
    let cases = [
        ("minimal", r#"{ "tmdb_movie_id": 27205 }"#),
        ("w2000", r#"{ "tmdb_movie_id": 27205, "width": "w2000" }"#),
        (
            "cinematic-with-badges",
            r##"{ "tmdb_movie_id": 27205,
                 "preset": "cinematic",
                 "badges": [{ "text": "#17 IMDb" }] }"##,
        ),
    ];

    let rendered = cases
        .iter()
        .map(|(name, json)| format!("{name}: {}", resolve(json).cache_key().to_hex()))
        .collect::<Vec<_>>()
        .join("\n");

    assert_snapshot!(rendered);
}
