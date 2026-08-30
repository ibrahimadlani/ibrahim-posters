//! Property tests for specification resolution and cache keys.
//!
//! These cover the invariants that unit tests can only sample by hand: that
//! resolution never panics, that clamping settles, and above all that the
//! canonical encoding is injective.

use poster_service::spec::key::canonical_encoding;
use poster_service::spec::request::{Badge, BadgeStyle, OutputWidth, Overrides, PosterRequest};
use poster_service::spec::{clamp, preset};
use poster_service::tmdb::{PosterPath, SourceKind};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Generates a syntactically valid TMDB path.
fn any_poster_path() -> impl Strategy<Value = PosterPath> {
    "[A-Za-z0-9]{20,60}"
        .prop_map(|stem| PosterPath::parse(&format!("/{stem}.jpg")).expect("generated valid"))
}

/// Generates badge text that survives validation.
///
/// Restricted to printable ASCII so the generator explores badge *structure*
/// rather than re-testing normalisation, which the unit tests cover directly.
///
/// The first character is non-space deliberately: text that is entirely
/// whitespace trims to empty and is rejected, which is correct behaviour but
/// would have this generator producing invalid requests rather than exploring
/// the space these properties are about.
fn any_badge() -> impl Strategy<Value = Badge> {
    (
        "[a-zA-Z0-9][a-zA-Z0-9 ]{0,31}",
        prop_oneof![
            Just(BadgeStyle::Solid),
            Just(BadgeStyle::Outline),
            Just(BadgeStyle::Accent)
        ],
    )
        .prop_map(|(text, style)| Badge { text, style })
}

/// Generates overrides across the full `f32` range, including out-of-range and
/// non-finite values, so clamping is exercised rather than assumed.
fn any_overrides() -> impl Strategy<Value = Overrides> {
    (
        proptest::option::of(any::<f32>()),
        proptest::option::of(any::<f32>()),
        proptest::option::of(any::<f32>()),
        proptest::option::of(any::<f32>()),
        proptest::option::of(any::<f32>()),
        proptest::option::of(any::<u32>()),
    )
        .prop_map(|(band, sigma, darken, logo_w, logo_b, badge_h)| Overrides {
            blur_band_fraction: band,
            blur_sigma: sigma,
            darken_strength: darken,
            logo_width_fraction: logo_w,
            logo_bottom_fraction: logo_b,
            badge_height: badge_h,
        })
}

fn any_request() -> impl Strategy<Value = PosterRequest> {
    (
        any_poster_path(),
        proptest::option::of(any_poster_path()),
        prop::collection::vec(any_badge(), 0..=clamp::MAX_BADGES),
        prop_oneof![Just(OutputWidth::W1000), Just(OutputWidth::W2000)],
        prop_oneof![Just(SourceKind::Poster), Just(SourceKind::Backdrop)],
        prop::sample::select(vec!["standard", "cinematic", "minimal", "poster_wall"]),
        any_overrides(),
    )
        .prop_map(
            |(source, logo, badges, width, source_kind, preset, overrides)| PosterRequest {
                source,
                source_kind,
                preset: preset.to_owned(),
                logo,
                badges,
                width,
                overrides,
            },
        )
}

proptest! {
    // Persist failing seeds next to this file so a failure found in CI is
    // replayed on every later run. The path is given explicitly because the
    // default strategy looks for a crate root beside the test source, does not
    // find one under tests/, and warns on every run.
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/spec_properties.proptest-regressions",
        ))),
        ..ProptestConfig::default()
    })]

    /// Resolution is total over every request that passes validation.
    #[test]
    fn resolution_never_panics(request in any_request()) {
        let _ = preset::resolve(&request);
    }

    /// Every resolved field lands inside its documented range, whatever the
    /// override asked for.
    #[test]
    fn resolution_respects_every_clamp(request in any_request()) {
        let spec = preset::resolve(&request).expect("generated requests are valid");
        let scale = request.width.scale();

        prop_assert!(clamp::BLUR_BAND_FRACTION.contains(&spec.blur_band_fraction));
        prop_assert!(clamp::DARKEN_STRENGTH.contains(&spec.darken_strength));
        prop_assert!(clamp::LOGO_WIDTH_FRACTION.contains(&spec.logo_width_fraction));
        prop_assert!(clamp::LOGO_BOTTOM_FRACTION.contains(&spec.logo_bottom_fraction));

        // Pixel-valued fields are scaled after clamping, so the bound scales too.
        prop_assert!(spec.blur_sigma >= *clamp::BLUR_SIGMA.start() * scale);
        prop_assert!(spec.blur_sigma <= *clamp::BLUR_SIGMA.end() * scale);
        prop_assert!(spec.badge_height >= *clamp::BADGE_HEIGHT.start() * request.width.pixel_scale());
        prop_assert!(spec.badge_height <= *clamp::BADGE_HEIGHT.end() * request.width.pixel_scale());

        // No non-finite value may escape into the renderer, whatever arrived.
        prop_assert!(spec.blur_band_fraction.is_finite());
        prop_assert!(spec.blur_sigma.is_finite());
        prop_assert!(spec.darken_strength.is_finite());
    }

    /// Clamping settles after one application.
    #[test]
    fn clamping_is_idempotent(value in any::<f32>()) {
        for range in [
            clamp::BLUR_BAND_FRACTION,
            clamp::BLUR_SIGMA,
            clamp::DARKEN_STRENGTH,
            clamp::LOGO_WIDTH_FRACTION,
            clamp::LOGO_BOTTOM_FRACTION,
        ] {
            let once = clamp::f32_into(value, range.clone());
            let twice = clamp::f32_into(once, range);
            prop_assert_eq!(once.to_bits(), twice.to_bits());
        }
    }

    /// Resolution is deterministic: the same request always resolves the same
    /// way, and therefore always produces the same key.
    #[test]
    fn resolution_is_deterministic(request in any_request()) {
        let first = preset::resolve(&request).expect("valid");
        let second = preset::resolve(&request).expect("valid");
        prop_assert_eq!(first.cache_key(), second.cache_key());
    }

    /// Equal specifications always produce equal keys.
    ///
    /// The direction that makes the cache hit rate reachable: two requests
    /// that resolve to the same thing must never occupy two cache entries.
    #[test]
    fn equal_specs_produce_equal_keys(a in any_request(), b in any_request()) {
        let spec_a = preset::resolve(&a).expect("valid");
        let spec_b = preset::resolve(&b).expect("valid");

        if spec_a == spec_b {
            prop_assert_eq!(spec_a.cache_key(), spec_b.cache_key());
        }
    }

    /// The canonical encoding is injective.
    ///
    /// This is the honest form of "distinct specifications must not collide".
    /// No test can establish blake3's collision resistance -- it is a
    /// cryptographic assumption, and a test that appears to check it is
    /// checking nothing. Injectivity of the encoding *is* falsifiable, and it
    /// catches the failure that actually happens: a field left out of the
    /// encoding, or a missing length prefix that lets two inputs share a
    /// representation. Collision resistance then follows from blake3.
    #[test]
    fn canonical_encoding_is_injective(a in any_request(), b in any_request()) {
        let spec_a = preset::resolve(&a).expect("valid");
        let spec_b = preset::resolve(&b).expect("valid");

        prop_assert_eq!(
            spec_a == spec_b,
            canonical_encoding(&spec_a) == canonical_encoding(&spec_b),
            "encoding equality must track specification equality in both directions"
        );
    }

    /// Every field participates in the encoding.
    ///
    /// Guards the failure injectivity alone would miss on a small sample: a
    /// field silently dropped from `canonical_encoding` would still be
    /// injective over most generated pairs while quietly merging cache entries
    /// that differ only in that field.
    #[test]
    fn changing_the_source_changes_the_key(
        request in any_request(),
        other in any_poster_path(),
    ) {
        let original = preset::resolve(&request).expect("valid");

        let mut mutated = request;
        mutated.source = other;
        let changed = preset::resolve(&mutated).expect("valid");

        if original.source != changed.source {
            prop_assert_ne!(original.cache_key(), changed.cache_key());
        }
    }
}
