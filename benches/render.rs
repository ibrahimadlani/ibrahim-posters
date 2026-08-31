//! Render benchmarks.
//!
//! The blur group is the point of this file. `blur::DOWNSCALE` is set to 8 on
//! the argument that downscaling divides both pixel count and required radius;
//! these benchmarks measure that claim across factors, alongside a
//! full-resolution baseline, so the constant stays evidence-backed rather than
//! inherited.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use poster_service::render::{self, blur, encode, source::Rgba8};
use poster_service::spec::{preset, request::PosterRequest, ResolvedSpec};
use std::hint::black_box;

/// Poster-shaped RGBA with structure a blur can act on.
fn synthetic(width: u32, height: u32) -> Rgba8 {
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
    Rgba8::new(pixels, width, height).expect("dimensions match")
}

fn spec_from(json: &str) -> ResolvedSpec {
    use poster_service::tmdb::api::{ArtworkOption, Catalogue, MediaKind};
    use poster_service::tmdb::PosterPath;

    let request: PosterRequest = serde_json::from_str(json).expect("valid request");
    let catalogue = Catalogue {
        kind: MediaKind::Movie,
        id: 27205,
        posters: vec![ArtworkOption {
            path: PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg").expect("valid"),
            language: Some("en".to_owned()),
            vote_average: 8.0,
            vote_count: 100,
            width: 2000,
            height: 3000,
        }],
        logos: Vec::new(),
    };
    preset::resolve(&request, &catalogue).expect("resolves")
}

/// Blur cost against downscale factor.
///
/// Factor 1 is the baseline the optimisation is measured against: a box blur
/// at full band resolution with the full sigma.
fn bench_blur(c: &mut Criterion) {
    // The band at w1000 with the standard preset: 30% of 1500 rows.
    let (width, height) = (1000_u32, 450_u32);
    let sigma = 24.0_f32;
    let band = synthetic(width, height);

    let mut group = c.benchmark_group("blur_band_w1000");
    group.throughput(Throughput::Elements(u64::from(width) * u64::from(height)));

    for factor in [1_u32, 2, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(factor),
            &factor,
            |bencher, &factor| {
                bencher.iter(|| {
                    let mut working = if factor == 1 {
                        band.clone()
                    } else {
                        source::resize(&band, (width / factor).max(1), (height / factor).max(1))
                            .expect("resizes")
                    };

                    blur::blur_rgba(
                        &mut working.pixels,
                        working.width,
                        working.height,
                        sigma / factor as f32,
                    );

                    if factor != 1 {
                        working = source::resize(&working, width, height).expect("resizes");
                    }
                    black_box(working)
                });
            },
        );
    }
    group.finish();
}

use poster_service::render::source;

/// Resize cost, the second most expensive stage.
fn bench_resize(c: &mut Criterion) {
    let source_image = synthetic(1500, 2250);
    c.bench_function("resize_to_fill_w1000", |bencher| {
        bencher.iter(|| {
            black_box(
                source::resize_to_fill(black_box(&source_image), 1000, 1500).expect("resizes"),
            )
        });
    });
}

/// Encode cost at delivery quality.
fn bench_encode(c: &mut Criterion) {
    let poster = synthetic(1000, 1500);
    c.bench_function("encode_webp_q82_w1000", |bencher| {
        bencher.iter(|| {
            black_box(encode::webp(black_box(&poster), encode::DELIVERY_QUALITY).expect("encodes"))
        });
    });
}

/// The whole pipeline, which is the number the latency budget is stated for.
fn bench_end_to_end(c: &mut Criterion) {
    let background =
        encode::webp(&synthetic(1200, 1800), encode::INTERMEDIATE_QUALITY).expect("encodes");

    let cases = [
        ("bare", r#"{ "tmdb_movie_id": 27205 }"#),
        (
            "with_badges",
            r##"{ "tmdb_movie_id": 27205,
                  "badges": [{ "text": "#17 IMDb", "style": "accent" },
                             { "text": "Oscar Nominee", "style": "outline" }] }"##,
        ),
        ("w2000", r#"{ "tmdb_movie_id": 27205, "width": "w2000" }"#),
    ];

    let mut group = c.benchmark_group("render_end_to_end");
    for (name, json) in cases {
        let spec = spec_from(json);
        let assets = render::Assets {
            background: background.clone(),
            logo: None,
        };
        group.bench_function(name, |bencher| {
            bencher.iter(|| black_box(render::render(black_box(&spec), black_box(&assets))));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_blur,
    bench_resize,
    bench_encode,
    bench_end_to_end
);
criterion_main!(benches);
