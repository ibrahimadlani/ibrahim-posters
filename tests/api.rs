//! Full HTTP round trips against a stubbed TMDB and in-memory storage.
//!
//! No network, no containers, no credentials: the whole API surface is
//! exercised through `tower::ServiceExt::oneshot` against the same router
//! `main` serves.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt as _;
use poster_service::config::Config;
use poster_service::render::{encode, source::Rgba8};
use poster_service::state::AppState;
use poster_service::storage::Storage;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Synthetic upstream artwork.
fn artwork(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x * 200 / width.max(1)) as u8,
                (y * 180 / height.max(1)) as u8,
                140,
                255,
            ]);
        }
    }
    encode::webp(
        &Rgba8::new(pixels, width, height).expect("dimensions match"),
        encode::INTERMEDIATE_QUALITY,
    )
    .expect("encodes")
}

/// A wide, fully opaque white logo.
///
/// Opaque white against a mid-tone background so its drawn extent can be
/// measured from the rendered poster, and 5:1 so a resize that ignores aspect
/// ratio is unmistakable.
fn logo_artwork() -> Vec<u8> {
    let (width, height) = (1000_u32, 200_u32);
    let pixels = vec![255_u8; (width * height * 4) as usize];
    let image = image::RgbaImage::from_raw(width, height, pixels).expect("dimensions match");
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encodes");
    out.into_inner()
}

/// A running stub CDN plus the state pointed at it.
struct Harness {
    state: AppState,
    upstream: MockServer,
}

impl Harness {
    async fn new() -> Self {
        Self::with_response(ResponseTemplate::new(200).set_body_bytes(artwork(1200, 1800))).await
    }

    async fn with_response(response: ResponseTemplate) -> Self {
        let upstream = MockServer::start().await;
        // Logos are matched first so a request for one is not answered with
        // background artwork; the fallback below covers everything else.
        Mock::given(method("GET"))
            .and(path_regex(r"^/w\d+/a+\.png$"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(logo_artwork()))
            .mount(&upstream)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/w\d+/.*"))
            .respond_with(response)
            .mount(&upstream)
            .await;

        let mut config = Config::defaults();
        config.tmdb_image_base = upstream.uri();
        config.public_base_url = Some("https://cdn.example.test".to_owned());

        let state = AppState::new(config, Storage::in_memory()).expect("state builds");
        Self { state, upstream }
    }

    fn router(&self) -> axum::Router {
        poster_service::api::router(&self.state)
    }

    async fn post(&self, body: Value) -> (StatusCode, Value) {
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/posters")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collects")
            .to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, uri: &str) -> axum::response::Response {
        self.router()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds")
    }
}

fn valid_request() -> Value {
    json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg" })
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn a_poster_round_trips_from_post_to_get() {
    let harness = Harness::new().await;

    let (status, created) = harness.post(valid_request()).await;
    assert_eq!(status, StatusCode::CREATED);

    let key = created["key"].as_str().expect("key is a string");
    assert_eq!(key.len(), 64, "key is not a 64-character digest");
    assert_eq!(
        created["url"],
        format!("https://cdn.example.test/v1/posters/{key}.webp")
    );

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(response.headers()["x-cache"], "MISS");

    let bytes = body_bytes(response).await;
    let probed = poster_service::tmdb::probe::probe(&bytes).expect("output is an image");
    assert_eq!((probed.width, probed.height), (1000, 1500));
}

#[tokio::test]
async fn a_second_request_for_one_key_is_served_from_cache() {
    let harness = Harness::new().await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key").to_owned();
    let uri = format!("/v1/posters/{key}.webp");

    let first = harness.get(&uri).await;
    assert_eq!(first.headers()["x-cache"], "MISS");
    let first_bytes = body_bytes(first).await;

    let second = harness.get(&uri).await;
    assert_eq!(second.headers()["x-cache"], "HIT");
    let second_bytes = body_bytes(second).await;

    assert_eq!(
        first_bytes, second_bytes,
        "cached bytes differ from rendered"
    );
}

#[tokio::test]
async fn equivalent_requests_converge_on_one_key() {
    // The property the cache hit rate depends on. An override that restates a
    // preset default, and one that clamps to it, must not create extra
    // entries.
    let harness = Harness::new().await;

    let (_, implicit) = harness.post(valid_request()).await;
    let (_, explicit) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "preset": "standard",
            "overrides": { "blur_sigma": 24.0 }
        }))
        .await;
    let (_, clamped) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "overrides": { "darken_strength": 99.0 }
        }))
        .await;
    let (_, at_cap) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "overrides": { "darken_strength": 0.95 }
        }))
        .await;

    assert_eq!(implicit["key"], explicit["key"]);
    assert_eq!(clamped["key"], at_cap["key"]);
}

#[tokio::test]
async fn every_render_fetches_its_source_from_upstream() {
    // The service stores what it produces, not what it consumes. Three
    // posters built from one piece of artwork fetch that artwork three times,
    // because none of it is kept between renders.
    //
    // This is the deliberate cost of not persisting source artwork; the
    // saving is that only rendered output is ever written to storage. See
    // docs/adr/0007-do-not-persist-source-artwork.md.
    let harness = Harness::new().await;

    for preset in ["standard", "cinematic", "minimal"] {
        let (_, created) = harness
            .post(json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "preset": preset }))
            .await;
        let key = created["key"].as_str().expect("key");
        let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
        assert_eq!(response.status(), StatusCode::OK, "{preset} failed");
    }

    let upstream_requests = harness
        .upstream
        .received_requests()
        .await
        .expect("recorded");
    assert_eq!(
        upstream_requests.len(),
        3,
        "expected one fetch per render, got {}",
        upstream_requests.len()
    );
}

#[tokio::test]
async fn a_cached_poster_costs_no_upstream_fetch() {
    // The saving that makes the above acceptable: a render happens once per
    // key, so repeat requests reach neither TMDB nor the renderer.
    let harness = Harness::new().await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key");
    let uri = format!("/v1/posters/{key}.webp");

    assert_eq!(harness.get(&uri).await.status(), StatusCode::OK);
    let after_first = harness
        .upstream
        .received_requests()
        .await
        .expect("recorded")
        .len();

    for _ in 0..5 {
        let response = harness.get(&uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-cache"], "HIT");
    }

    let after_repeats = harness
        .upstream
        .received_requests()
        .await
        .expect("recorded")
        .len();
    assert_eq!(
        after_first, after_repeats,
        "a cached poster reached upstream"
    );
}

#[tokio::test]
async fn an_unknown_key_is_not_found_and_briefly_cacheable() {
    let harness = Harness::new().await;
    let response = harness
        .get(&format!("/v1/posters/{}.webp", "ab".repeat(32)))
        .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // A bad link being shared produces a retry storm the CDN should absorb.
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=60"
    );
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn a_malformed_key_is_not_found() {
    let harness = Harness::new().await;

    for uri in [
        "/v1/posters/short.webp",
        "/v1/posters/nothex_nothex_nothex_nothex_nothex_nothex_nothex_nothex_zz.webp",
        "/v1/posters/../../etc/passwd",
    ] {
        let response = harness.get(uri).await;
        assert!(
            response.status() == StatusCode::NOT_FOUND,
            "{uri} returned {}",
            response.status()
        );
    }
}

#[tokio::test]
async fn a_key_without_the_extension_is_not_found() {
    // The extension is part of the route, not decoration: a future AVIF
    // variant is a different extension on the same key.
    let harness = Harness::new().await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_invalid_poster_path_is_rejected_with_its_code() {
    let harness = Harness::new().await;

    for source in [
        "https://evil.test/aaaaaaaaaaaaaaaaaaaa.jpg",
        "//evil.test/aaaaaaaaaaaaaaaaaaaa.jpg",
        "/../../etc/passwd",
        "/short.jpg",
    ] {
        let (status, body) = harness.post(json!({ "source": source })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{source} was accepted");
        assert_eq!(
            body["code"], "invalid_poster_path",
            "{source} should report the path code, not a generic parse failure"
        );
        // The detail names the rule that was broken, so a caller can fix it
        // without guessing.
        assert!(
            body["detail"].as_str().expect("detail").len() > 20,
            "{source} produced an unhelpful detail"
        );
    }
}

#[tokio::test]
async fn an_unknown_preset_is_reported_by_code() {
    let harness = Harness::new().await;
    let (status, body) = harness
        .post(json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "preset": "nope" }))
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_preset");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn too_many_badges_are_rejected() {
    let harness = Harness::new().await;
    let badges: Vec<Value> = (0..12)
        .map(|i| json!({ "text": format!("badge {i}") }))
        .collect();

    let (status, body) = harness
        .post(json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "badges": badges }))
        .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_failed");
}

#[tokio::test]
async fn a_bad_logo_path_is_reported_as_a_path_error() {
    let harness = Harness::new().await;
    let (status, body) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "logo": "https://evil.test/x.png"
        }))
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_poster_path");
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_parse_error() {
    let harness = Harness::new().await;
    let response = harness
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/posters")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{ not json"))
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&body_bytes(response).await).expect("json");
    assert_eq!(body["code"], "malformed_request");
}

#[tokio::test]
async fn an_unknown_field_is_rejected_rather_than_ignored() {
    // deny_unknown_fields all the way to the wire: a caller who misspells
    // "preset" should learn about it rather than silently get the default.
    let harness = Harness::new().await;
    let (status, body) = harness
        .post(json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "presset": "cinematic" }))
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "malformed_request");
}

#[tokio::test]
async fn a_missing_upstream_source_surfaces_as_not_found() {
    let harness = Harness::with_response(ResponseTemplate::new(404)).await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body: Value = serde_json::from_slice(&body_bytes(response).await).expect("json");
    assert_eq!(body["code"], "source_not_found");
    assert_eq!(body["retryable"], false, "a retry cannot make it exist");
}

#[tokio::test]
async fn an_upstream_outage_is_retryable() {
    let harness = Harness::with_response(ResponseTemplate::new(503)).await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(response.headers().contains_key(header::RETRY_AFTER));

    let body: Value = serde_json::from_slice(&body_bytes(response).await).expect("json");
    assert_eq!(body["code"], "upstream_unavailable");
    assert_eq!(body["retryable"], true);
}

#[tokio::test]
async fn a_decompression_bomb_is_rejected_before_rendering() {
    // A 24-byte PNG header declaring a 14 GB decode target.
    let mut bomb = b"\x89PNG\r\n\x1a\n".to_vec();
    bomb.extend_from_slice(&13_u32.to_be_bytes());
    bomb.extend_from_slice(b"IHDR");
    bomb.extend_from_slice(&60_000_u32.to_be_bytes());
    bomb.extend_from_slice(&60_000_u32.to_be_bytes());

    let harness = Harness::with_response(ResponseTemplate::new(200).set_body_bytes(bomb)).await;
    let (_, created) = harness.post(valid_request()).await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body: Value = serde_json::from_slice(&body_bytes(response).await).expect("json");
    assert_eq!(body["code"], "source_dimensions_exceeded");
}

#[tokio::test]
async fn an_oversized_request_body_is_rejected() {
    let harness = Harness::new().await;
    let response = harness
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/posters")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b'x'; 64 * 1024]))
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert!(
        response.status() == StatusCode::PAYLOAD_TOO_LARGE
            || response.status() == StatusCode::BAD_REQUEST,
        "oversized body returned {}",
        response.status()
    );
}

#[tokio::test]
async fn the_preset_catalogue_lists_every_preset() {
    let harness = Harness::new().await;
    let response = harness.get("/v1/presets").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(&body_bytes(response).await).expect("json");
    let presets = body["presets"].as_array().expect("presets is an array");

    assert_eq!(presets.len(), 4);
    // Resolved values, not raw file contents: a client can predict what a
    // request produces without reimplementing the merge.
    let standard = presets
        .iter()
        .find(|p| p["name"] == "standard")
        .expect("standard");
    assert!(standard["blur_sigma"].is_number());
    assert!(standard["badge_height"].is_number());
}

#[tokio::test]
async fn w2000_renders_at_the_larger_size() {
    let harness = Harness::new().await;
    let (_, created) = harness
        .post(json!({ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg", "width": "w2000" }))
        .await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::OK);

    let probed = poster_service::tmdb::probe::probe(&body_bytes(response).await)
        .expect("output is an image");
    assert_eq!((probed.width, probed.height), (2000, 3000));
}

#[tokio::test]
async fn badges_and_a_logo_survive_the_round_trip() {
    let harness = Harness::new().await;
    let (status, created) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png",
            "preset": "cinematic",
            "badges": [
                { "text": "#17 IMDb", "style": "accent" },
                { "text": "Oscar Nominee", "style": "outline" }
            ]
        }))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let key = created["key"].as_str().expect("key");
    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_logo_keeps_its_aspect_ratio_through_the_http_path() {
    // Regression: `gather_assets` originally loaded the logo through the same
    // helper as the background, which resizes to *fill the poster frame*. The
    // logo arrived pre-cropped to 2:3 and the renderer then fitted whatever
    // survived, drawing a narrow rectangle instead of a wide wordmark.
    //
    // Nothing caught it: the API tests asserted status codes, and the visual
    // regression suite calls `render` directly with correctly prepared assets,
    // so it never exercised asset loading. This measures drawn geometry
    // through the full HTTP path.
    let harness = Harness::new().await;
    let (_, created) = harness
        .post(json!({
            "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
            "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png"
        }))
        .await;
    let key = created["key"].as_str().expect("key");

    let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
    assert_eq!(response.status(), StatusCode::OK);

    let decoded = poster_service::render::source::decode(&body_bytes(response).await)
        .expect("output decodes");

    // Find the extent of near-white pixels: the logo is the only pure white
    // element outside the badge row, and no badges were requested.
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (u32::MAX, 0_u32, u32::MAX, 0_u32);
    for y in 0..decoded.height {
        for x in 0..decoded.width {
            let index = ((y * decoded.width + x) * 4) as usize;
            let pixel = &decoded.pixels[index..index + 3];
            if pixel.iter().all(|&channel| channel > 235) {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }

    assert!(max_x > min_x, "no logo was drawn at all");
    let drawn_width = max_x - min_x + 1;
    let drawn_height = max_y - min_y + 1;

    // standard preset: 60% of 1000 px wide, at the source's 5:1 ratio.
    assert!(
        (580..=620).contains(&drawn_width),
        "logo drawn {drawn_width} px wide, expected about 600"
    );

    let ratio = f64::from(drawn_width) / f64::from(drawn_height);
    assert!(
        (ratio - 5.0).abs() < 0.4,
        "logo aspect ratio is {ratio:.2}, expected about 5.0 -- it was reshaped"
    );
}

#[tokio::test]
async fn a_logo_is_fetched_for_each_render_and_never_stored() {
    // Logos follow the same rule as backgrounds: fetched per render, never
    // written to storage.
    let harness = Harness::new().await;

    for preset in ["standard", "cinematic"] {
        let (_, created) = harness
            .post(json!({
                "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                "logo": "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png",
                "preset": preset
            }))
            .await;
        let key = created["key"].as_str().expect("key");
        let response = harness.get(&format!("/v1/posters/{key}.webp")).await;
        assert_eq!(response.status(), StatusCode::OK, "{preset} failed");
    }

    let requests = harness
        .upstream
        .received_requests()
        .await
        .expect("recorded");
    let logo_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with(".png"))
        .count();

    assert_eq!(
        logo_requests, 2,
        "expected one logo fetch per render, got {logo_requests}"
    );
}
