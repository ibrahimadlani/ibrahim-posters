//! Visual regression: renders fixed fixtures and compares against committed
//! reference images.
//!
//! # How the RENDER_VERSION gate works
//!
//! References live in `tests/visual/reference/v{RENDER_VERSION}/`. That path
//! is what makes the gate self-enforcing rather than a matter of discipline:
//!
//! - Change the renderer without bumping the constant, and the current
//!   version's references no longer match. CI fails.
//! - Bump the constant, and its directory does not exist yet. CI fails, with
//!   instructions to regenerate.
//!
//! Either way a rendering change cannot reach `main` without someone
//! deliberately deciding what happens to the cache. That matters because
//! posters are served with a one-year `immutable` directive and there is no
//! invalidation path — a renderer change that keeps its keys serves stale
//! pixels for a year with no way to correct it.
//!
//! # Why pre-encode pixels
//!
//! References are lossless PNG compared against the RGBA buffer, never
//! against WebP bytes. libwebp output is not bit-stable across library
//! versions, so comparing encoded bytes would fail on a libwebp bump that
//! changed nothing visible.
//!
//! # Why the comparison is tiled
//!
//! `fast_image_resize` dispatches to different SIMD paths depending on the
//! CPU, so exact equality is unachievable across machines and some tolerance
//! is required.
//!
//! A whole-image mean is the obvious tolerance and it is not sufficient. It
//! was tried first and measured against a deliberately broken renderer:
//! moving the badge row by 45 px — an obvious regression — produced a global
//! mean error of **0.967**, comfortably inside any threshold loose enough to
//! absorb SIMD noise. A localised change is diluted by every pixel it did not
//! touch, and a moved element is precisely the kind of regression this gate
//! exists to catch.
//!
//! So the image is divided into a 16×24 grid and *each tile* must agree. The
//! same badge move produces a worst-tile mean error of **57.4**, nearly sixty
//! times the threshold. The global mean is kept as a second check because it
//! catches the opposite failure: a small shift spread across the whole frame,
//! such as a darkening ramp that is 3 % too strong, which no single tile
//! would flag.
//!
//! Regenerate with: `VISUAL_UPDATE=1 cargo test --test visual_regression`

use std::path::{Path, PathBuf};

use poster_service::render::{self, source::Rgba8};
use poster_service::spec::{preset, request::PosterRequest, ResolvedSpec};
use poster_service::RENDER_VERSION;

/// Largest tolerated whole-image mean absolute error, in 0-255 units.
///
/// Catches uniform drift. A darkening ramp 3 % too strong measures 1.86 here,
/// so this threshold rejects it.
const MAX_MEAN_ERROR: f64 = 1.5;

/// Largest tolerated per-tile mean absolute error.
///
/// Catches localised change. Measured against a badge row moved 45 px, which
/// produces 57.4 — so this leaves a factor of seven of headroom while sitting
/// far above the sub-unit disagreement SIMD dispatch produces.
const MAX_TILE_ERROR: f64 = 8.0;

/// Grid the comparison is tiled over: columns, then rows.
///
/// At w1000 each tile is roughly 62 px square, which is smaller than any
/// element the renderer draws. A grid coarser than the elements would dilute
/// a moved element the same way a whole-image mean does.
const GRID: (u32, u32) = (16, 24);

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/visual/reference")
        .join(format!("v{RENDER_VERSION}"))
}

/// Deterministic synthetic background.
///
/// Synthetic rather than real TMDB artwork so nothing copyrighted enters the
/// repository, and deterministic so the fixture itself can never be the
/// reason a comparison fails.
fn background(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // A checkerboard over a gradient: the gradient exercises the
            // ramps, the checker exercises the blur. A flat fill would let a
            // broken blur pass unnoticed.
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
        render::encode::INTERMEDIATE_QUALITY,
    )
    .expect("encodes")
}

/// Deterministic synthetic logo with a real alpha channel.
fn logo(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let inside = y > height / 5
                && y < height - height / 5
                && x > width / 20
                && x < width - width / 20
                && (x / (width / 14).max(1)).is_multiple_of(2);
            pixels.extend_from_slice(if inside {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    let image = image::RgbaImage::from_raw(width, height, pixels).expect("dimensions match");
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encodes");
    out.into_inner()
}

fn spec_from(json: &str) -> ResolvedSpec {
    let request: PosterRequest = serde_json::from_str(json).expect("valid request");
    preset::resolve(&request).expect("resolves")
}

/// Renders one case and compares it against its reference.
fn check(name: &str, json: &str, with_logo: bool) {
    let spec = spec_from(json);
    let (width, height) = spec.dimensions();

    let assets = render::Assets {
        background: background(width + 200, height + 300),
        logo: with_logo.then(|| logo(900, 220)),
    };

    let rendered = render::render(&spec, &assets).expect("renders");

    let dir = reference_dir();
    let path = dir.join(format!("{name}.png"));

    if std::env::var_os("VISUAL_UPDATE").is_some() {
        std::fs::create_dir_all(&dir).expect("reference directory is creatable");
        image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.pixels.clone())
            .expect("dimensions match")
            .save(&path)
            .expect("reference saves");
        return;
    }

    assert!(
        dir.exists(),
        "no reference images for RENDER_VERSION {RENDER_VERSION}.\n\
         Either the constant was bumped without regenerating, or rendering \
         changed and the bump is what is missing.\n\
         Regenerate with: VISUAL_UPDATE=1 cargo test --test visual_regression"
    );
    assert!(path.exists(), "missing reference {}", path.display());

    let reference = image::open(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
        .into_rgba8();

    assert_eq!(
        (reference.width(), reference.height()),
        (rendered.width, rendered.height),
        "{name}: output size changed"
    );

    let difference = compare(&reference, &rendered.pixels);

    assert!(
        difference.mean_error <= MAX_MEAN_ERROR && difference.worst_tile <= MAX_TILE_ERROR,
        "{name}: rendered output differs from its reference.\n  \
         whole-image mean error {:.3} (limit {MAX_MEAN_ERROR})\n  \
         worst tile mean error  {:.3} (limit {MAX_TILE_ERROR}) at tile {:?}\n\n\
         If this change to rendering is intended, bump RENDER_VERSION in \
         src/lib.rs and regenerate the references with \
         VISUAL_UPDATE=1 cargo test --test visual_regression.\n\
         Shipping a rendering change without that bump serves stale posters \
         behind a one-year immutable header, and there is no way to correct \
         it afterwards.",
        difference.mean_error,
        difference.worst_tile,
        difference.worst_tile_at,
    );
}

/// How far a rendered image is from its reference.
#[derive(Debug)]
struct Difference {
    /// Mean absolute error over every channel of every pixel.
    mean_error: f64,
    /// Largest per-tile mean absolute error.
    worst_tile: f64,
    /// Grid position of that tile, column then row.
    worst_tile_at: (u32, u32),
}

/// Compares whole-image and per-tile.
fn compare(reference: &image::RgbaImage, rendered: &[u8]) -> Difference {
    let expected = reference.as_raw();
    let width = reference.width();
    let height = reference.height();

    let total: u64 = expected
        .iter()
        .zip(rendered)
        .map(|(left, right)| u64::from(left.abs_diff(*right)))
        .sum();

    let (columns, rows) = GRID;
    let tile_width = (width / columns).max(1);
    let tile_height = (height / rows).max(1);

    let mut worst_tile = 0.0_f64;
    let mut worst_tile_at = (0, 0);

    for row in 0..rows {
        for column in 0..columns {
            let mut sum = 0_u64;
            let mut samples = 0_u64;

            for y in (row * tile_height)..((row + 1) * tile_height).min(height) {
                for x in (column * tile_width)..((column + 1) * tile_width).min(width) {
                    let index = ((y * width + x) * 4) as usize;
                    for channel in 0..4 {
                        sum += u64::from(
                            expected[index + channel].abs_diff(rendered[index + channel]),
                        );
                        samples += 1;
                    }
                }
            }

            if samples == 0 {
                continue;
            }
            let mean = sum as f64 / samples as f64;
            if mean > worst_tile {
                worst_tile = mean;
                worst_tile_at = (column, row);
            }
        }
    }

    Difference {
        mean_error: total as f64 / expected.len() as f64,
        worst_tile,
        worst_tile_at,
    }
}

#[test]
fn standard_preset_without_extras() {
    check(
        "standard-bare",
        r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" }"#,
        false,
    );
}

#[test]
fn standard_preset_with_logo_and_badges() {
    check(
        "standard-full",
        r##"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
              "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png",
              "badges": [
                { "text": "#17 IMDb", "style": "accent" },
                { "text": "Oscar Nominee", "style": "outline" },
                { "text": "4K HDR" }
              ] }"##,
        true,
    );
}

#[test]
fn cinematic_preset() {
    check(
        "cinematic",
        r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "preset": "cinematic" }"#,
        true,
    );
}

#[test]
fn minimal_preset() {
    check(
        "minimal",
        r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "preset": "minimal" }"#,
        true,
    );
}

#[test]
fn poster_wall_preset() {
    check(
        "poster-wall",
        r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "preset": "poster_wall" }"#,
        true,
    );
}

#[test]
fn overrides_are_visible_in_the_output() {
    // Guards the failure where overrides resolve correctly but no stage
    // consults them -- which every unit test in spec/ would still pass.
    check(
        "overrides",
        r#"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
             "overrides": { "blur_band_fraction": 0.55, "blur_sigma": 60.0,
                            "darken_strength": 0.9, "logo_width_fraction": 0.85 } }"#,
        true,
    );
}

#[test]
fn w2000_output() {
    check(
        "w2000",
        r##"{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
              "width": "w2000",
              "badges": [{ "text": "#17 IMDb", "style": "accent" }] }"##,
        true,
    );
}
