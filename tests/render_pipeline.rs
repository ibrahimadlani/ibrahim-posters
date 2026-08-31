//! End-to-end render behaviour.
//!
//! These assert *properties* of the output — that a stage ran, that geometry
//! responds to the specification — rather than exact pixels, which is what
//! `visual_regression` covers. The split matters: a property test says what
//! the renderer is supposed to do, and a reference image says what it
//! currently does.

mod support;

use poster_service::render::{self, source::Rgba8};
use poster_service::spec::{preset, request::PosterRequest, ResolvedSpec};

/// Synthetic background: a checkerboard over a gradient.
///
/// The checkerboard is what makes a broken blur detectable — a flat fill
/// would look identical blurred and unblurred.
fn background(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let checker = if ((x / 48) + (y / 48)) % 2 == 0 {
                45
            } else {
                0
            };
            pixels.extend_from_slice(&[
                (190 - (y * 140 / height)) as u8 + checker,
                (55 + (x * 130 / width)) as u8 + checker,
                (135 + (y * 95 / height)) as u8,
                255,
            ]);
        }
    }
    render::encode::webp(
        &Rgba8::new(pixels, width, height).expect("dimensions match"),
        90.0,
    )
    .expect("encodes")
}

fn spec_from(json: &str) -> ResolvedSpec {
    let request: PosterRequest = serde_json::from_str(json).expect("valid request");
    preset::resolve(&request, &support::catalogue()).expect("resolves")
}

fn render(json: &str) -> Rgba8 {
    let spec = spec_from(json);
    let (width, height) = spec.dimensions();
    let assets = render::Assets {
        background: background(width + 200, height + 300),
        logo: None,
    };
    render::render(&spec, &assets).expect("renders")
}

/// Mean luminance of one row.
fn luminance(image: &Rgba8, row: u32) -> f64 {
    let stride = (image.width * 4) as usize;
    let start = row as usize * stride;
    let total: u64 = image.pixels[start..start + stride]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
        .sum();
    total as f64 / (f64::from(image.width) * 3.0)
}

/// Mean absolute difference between horizontally adjacent pixels in one row.
///
/// A proxy for high-frequency detail. The checkerboard makes this large; a
/// working blur drives it toward zero.
fn detail(image: &Rgba8, row: u32) -> f64 {
    let stride = (image.width * 4) as usize;
    let start = row as usize * stride;
    let values: Vec<i32> = image.pixels[start..start + stride]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]))
        .collect();
    let total: i64 = values
        .windows(2)
        .map(|pair| i64::from((pair[1] - pair[0]).abs()))
        .sum();
    total as f64 / (values.len() - 1) as f64
}

#[test]
fn output_matches_the_requested_dimensions() {
    let small = render(r#"{ "tmdb_movie_id": 27205 }"#);
    assert_eq!((small.width, small.height), (1000, 1500));

    let large = render(r#"{ "tmdb_movie_id": 27205, "width": "w2000" }"#);
    assert_eq!((large.width, large.height), (2000, 3000));
}

#[test]
fn output_is_fully_opaque() {
    // The background fills the frame, so nothing may leave a transparent
    // pixel. A hole would encode as black and be invisible until someone
    // composited the poster over something.
    let poster = render(r#"{ "tmdb_movie_id": 27205 }"#);
    assert!(
        poster.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
        "output contains a transparent pixel"
    );
}

#[test]
fn the_blur_removes_detail_inside_the_band_only() {
    let poster = render(r#"{ "tmdb_movie_id": 27205 }"#);

    let above = detail(&poster, 400);
    let inside = detail(&poster, 1450);

    assert!(above > 1.0, "the fixture has no detail to remove: {above}");
    assert!(
        inside < above / 10.0,
        "blur left {inside} detail inside the band against {above} above it"
    );
}

#[test]
fn a_larger_sigma_removes_more_detail() {
    let weak = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "blur_sigma": 4.0 } }"#,
    );
    let strong = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "blur_sigma": 80.0 } }"#,
    );

    assert!(detail(&strong, 1450) < detail(&weak, 1450));
}

#[test]
fn a_zero_sigma_leaves_the_band_sharp() {
    // The band is still darkened; only the blur is skipped.
    let poster = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "blur_sigma": 0.0 } }"#,
    );
    assert!(
        detail(&poster, 1450) > 0.5,
        "sigma 0 should not have blurred anything"
    );
}

#[test]
fn the_darkening_ramp_darkens_toward_the_bottom() {
    let poster = render(r#"{ "tmdb_movie_id": 27205 }"#);

    let above = luminance(&poster, 400);
    let bottom = luminance(&poster, 1499);

    assert!(
        bottom < above * 0.6,
        "bottom luminance {bottom} against {above} above the band"
    );
}

#[test]
fn a_stronger_darkening_setting_darkens_more() {
    let light = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "darken_strength": 0.1 } }"#,
    );
    let heavy = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "darken_strength": 0.95 } }"#,
    );

    assert!(luminance(&heavy, 1499) < luminance(&light, 1499));
}

#[test]
fn the_band_fraction_controls_where_treatment_begins() {
    // A narrow band must leave detail at a row a wide band has already
    // blurred. Without this, a band fraction that resolved correctly but was
    // never consulted would go unnoticed.
    let narrow = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "blur_band_fraction": 0.10, "blur_sigma": 40.0 } }"#,
    );
    let wide = render(
        r#"{ "tmdb_movie_id": 27205,
             "overrides": { "blur_band_fraction": 0.55, "blur_sigma": 40.0 } }"#,
    );

    let probe_row = 1150;
    assert!(
        detail(&narrow, probe_row) > detail(&wide, probe_row) * 3.0,
        "band fraction did not move the treatment boundary"
    );
}

#[test]
fn badges_are_drawn_near_the_top() {
    let spec = spec_from(
        r##"{ "tmdb_movie_id": 27205,
              "badges": [{ "text": "Oscar Nominee", "style": "solid" }] }"##,
    );
    let assets = render::Assets {
        background: background(1200, 1800),
        logo: None,
    };
    let with_badges = render::render(&spec, &assets).expect("renders");

    let bare = render(r#"{ "tmdb_movie_id": 27205 }"#);

    // Averaged over the whole badge band rather than sampled at one row. A
    // solid badge is a white pill with near-black text, so a single row can
    // land on the text baseline and net out to the background luminance --
    // which is what a first version of this test did, and it reported a
    // missing badge that was in fact drawn correctly.
    let band: Vec<u32> = (67..111).collect();
    let brightened = band
        .iter()
        .filter(|row| luminance(&with_badges, **row) > luminance(&bare, **row) + 5.0)
        .count();

    assert!(
        brightened > band.len() / 2,
        "only {brightened} of {} badge rows brightened",
        band.len()
    );

    // And the text itself must be present: a white pill with no glyphs would
    // brighten the band just as much.
    let stride = (with_badges.width * 4) as usize;
    let dark_text = with_badges.pixels[67 * stride..111 * stride]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] < 70 && p[1] < 70 && p[2] < 70)
        .count();

    assert!(dark_text > 100, "badge pill drew but its text did not");
}

#[test]
fn rendering_is_deterministic() {
    // The cache serves one render for every request that resolves to the same
    // key, so two renders of one specification must agree exactly.
    let json = r##"{ "tmdb_movie_id": 27205,
                     "preset": "cinematic",
                     "badges": [{ "text": "#17 IMDb", "style": "accent" }] }"##;
    assert_eq!(render(json).pixels, render(json).pixels);
}

#[test]
fn corrupt_source_bytes_fail_rather_than_panic() {
    let spec = spec_from(r#"{ "tmdb_movie_id": 27205 }"#);

    for bytes in [
        Vec::new(),
        b"not an image".to_vec(),
        vec![0xFF, 0xD8, 0xFF, 0xE0],
    ] {
        let assets = render::Assets {
            background: bytes,
            logo: None,
        };
        assert!(
            render::render(&spec, &assets).is_err(),
            "corrupt input should fail cleanly"
        );
    }
}

#[test]
fn a_source_of_any_aspect_ratio_fills_the_frame() {
    // Sources are not 2:3. Fitting inside the frame would letterbox; filling
    // and cropping is what keeps the frame covered.
    let spec = spec_from(r#"{ "tmdb_movie_id": 27205 }"#);

    for (width, height) in [(3000_u32, 500_u32), (500, 3000), (1000, 1000)] {
        let assets = render::Assets {
            background: background(width, height),
            logo: None,
        };
        let poster = render::render(&spec, &assets).expect("renders");

        assert_eq!((poster.width, poster.height), (1000, 1500));
        assert!(
            poster.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "{width}x{height} source left a transparent region"
        );
    }
}

#[test]
fn render_encoded_produces_a_readable_webp() {
    use poster_service::tmdb::probe;

    let spec = spec_from(r#"{ "tmdb_movie_id": 27205 }"#);
    let assets = render::Assets {
        background: background(1200, 1800),
        logo: None,
    };

    let encoded = render::render_encoded(&spec, &assets).expect("renders and encodes");
    let probed = probe::probe(&encoded).expect("output is a recognisable image");

    assert_eq!(probed.format, probe::ImageFormat::WebP);
    assert_eq!((probed.width, probed.height), (1000, 1500));
}
