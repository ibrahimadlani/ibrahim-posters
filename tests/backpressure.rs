//! Concurrency behaviour of the render path.
//!
//! These drive many requests at once against a stubbed upstream, which is the
//! only way to observe single-flight and admission: both are invisible to a
//! sequential test.

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use poster_service::admission::{Admission, ADMISSION_TIMEOUT};
use poster_service::config::Config;
use poster_service::render::{encode, source::Rgba8};
use poster_service::state::AppState;
use poster_service::storage::Storage;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn artwork() -> Vec<u8> {
    let (width, height) = (1200_u32, 1800_u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x * 200 / width) as u8,
                (y * 180 / height) as u8,
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

/// Builds a harness whose upstream delays every response by `delay`.
///
/// The delay is what makes concurrency observable: without it a render
/// finishes before the next request arrives and every test looks sequential.
async fn harness(delay: Duration, concurrency: Option<usize>) -> (AppState, Arc<MockServer>) {
    let upstream = MockServer::start().await;

    // Metadata answers immediately: the delay belongs on artwork, which is
    // what these tests are timing.
    Mock::given(method("GET"))
        .and(path_regex(r"^/3/(movie|tv)/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(support::detail_body()))
        .mount(&upstream)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/3/(movie|tv)/\d+/images$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(support::images_body()))
        .mount(&upstream)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/w\d+/.*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(artwork())
                .set_delay(delay),
        )
        .mount(&upstream)
        .await;

    let mut config = Config::defaults();
    config.tmdb_image_base = upstream.uri();
    config.tmdb_api_base = upstream.uri();
    config.tmdb_api_key = Some(secrecy::SecretString::from("test-key"));
    config.render_concurrency = concurrency;

    let state = AppState::new(config, Storage::in_memory()).expect("state builds");
    (state, Arc::new(upstream))
}

async fn create(state: &AppState, body: Value) -> String {
    let response = poster_service::api::router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/posters")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    serde_json::from_slice::<Value>(&bytes).expect("json")["key"]
        .as_str()
        .expect("key")
        .to_owned()
}

/// Issues one GET and returns its status and `x-cache` value.
async fn fetch(state: AppState, key: String) -> (StatusCode, String) {
    let response = poster_service::api::router(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/v1/posters/{key}.webp"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    let status = response.status();
    let cache = response
        .headers()
        .get("x-cache")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("(none)")
        .to_owned();
    (status, cache)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_for_one_cold_key_fetch_upstream_once() {
    // The failure single-flight prevents: a cold key going viral producing one
    // render, and one upstream fetch, per concurrent request.
    let (state, upstream) = harness(Duration::from_millis(150), Some(8)).await;
    let key = create(&state, json!({ "tmdb_movie_id": 27205 })).await;

    let mut tasks = Vec::new();
    for _ in 0..12 {
        tasks.push(tokio::spawn(fetch(state.clone(), key.clone())));
    }

    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.expect("request completes"));
    }

    assert!(
        statuses.iter().all(|(status, _)| *status == StatusCode::OK),
        "not every concurrent request succeeded: {statuses:?}"
    );

    let fetches = upstream
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|request| request.url.path().starts_with("/w"))
        .count();
    // Two, not one: a render pulls a background and a logo. What matters is
    // that it is one render's worth rather than twelve.
    assert_eq!(
        fetches, 2,
        "twelve concurrent requests should cost one render's fetches, got {fetches}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coalesced_responses_are_labelled_distinctly() {
    // A rising HIT rate means the cache is working; a rising COALESCED rate
    // means concurrent demand for cold keys, which is a capacity signal. They
    // are different things and must not report the same way.
    let (state, _upstream) = harness(Duration::from_millis(150), Some(8)).await;
    let key = create(&state, json!({ "tmdb_movie_id": 27205 })).await;

    let mut tasks = Vec::new();
    for _ in 0..8 {
        tasks.push(tokio::spawn(fetch(state.clone(), key.clone())));
    }

    let mut labels = Vec::new();
    for task in tasks {
        labels.push(task.await.expect("request completes").1);
    }

    let rendered = labels.iter().filter(|label| *label == "MISS").count();
    let coalesced = labels.iter().filter(|label| *label == "COALESCED").count();

    assert_eq!(rendered, 1, "expected exactly one render, got {rendered}");
    assert!(coalesced > 0, "no request reported COALESCED: {labels:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_keys_render_concurrently() {
    // Single-flight must serialise duplicates, not unrelated work.
    //
    // Measured by comparing concurrent execution against sequential execution
    // of the same amount of work, rather than against an absolute deadline. An
    // absolute bound has to be calibrated against a build profile: the first
    // version of this test used one derived from release timings and failed in
    // CI, which runs debug, for reasons that had nothing to do with
    // concurrency. A ratio is immune to how fast the machine is.
    let (state, upstream) = harness(Duration::from_millis(50), Some(8)).await;

    const PRESETS: [&str; 4] = ["standard", "cinematic", "minimal", "poster_wall"];

    let sequential_specs: Vec<Value> = PRESETS
        .iter()
        .map(|preset| json!({ "tmdb_movie_id": 27205, "preset": preset }))
        .collect();

    // A distinct set of posters for the concurrent half, so the two
    // measurements share no cache entries and neither is served warm.
    let concurrent_specs: Vec<Value> = PRESETS
        .iter()
        .map(|preset| {
            json!({
                "tmdb_movie_id": 27205,
                "preset": preset,
                "overrides": { "blur_sigma": 31.0 }
            })
        })
        .collect();

    let mut sequential_keys = Vec::new();
    for spec in sequential_specs {
        sequential_keys.push(create(&state, spec).await);
    }
    let mut concurrent_keys = Vec::new();
    for spec in concurrent_specs {
        concurrent_keys.push(create(&state, spec).await);
    }

    // Both halves pay an identical upstream cost per request, so the
    // comparison still isolates concurrency: what differs between them is
    // only whether the work overlaps.
    let (status, _) = fetch(state.clone(), sequential_keys[0].clone()).await;
    assert_eq!(status, StatusCode::OK, "warm-up failed");

    let sequential_start = std::time::Instant::now();
    for key in &sequential_keys[1..] {
        let (status, _) = fetch(state.clone(), key.clone()).await;
        assert_eq!(status, StatusCode::OK);
    }
    let sequential = sequential_start.elapsed();

    let concurrent_start = std::time::Instant::now();
    let mut tasks = Vec::new();
    for key in &concurrent_keys[..3] {
        tasks.push(tokio::spawn(fetch(state.clone(), key.clone())));
    }
    for task in tasks {
        let (status, _) = task.await.expect("request completes");
        assert_eq!(status, StatusCode::OK);
    }
    let concurrent = concurrent_start.elapsed();

    assert!(
        concurrent < sequential.mul_f64(0.8),
        "three concurrent renders took {concurrent:?} against {sequential:?} sequential, \
         which is not the speed-up concurrency should give"
    );

    // One fetch per render, since source artwork is never persisted. What
    // single-flight still guarantees is that *duplicate* work is collapsed --
    // asserted by `concurrent_requests_for_one_cold_key_fetch_upstream_once`
    // -- not that unrelated posters share a fetch.
    let fetches = upstream
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|request| request.url.path().starts_with("/w"))
        .count();
    // Seven renders, two assets each.
    assert_eq!(
        fetches, 14,
        "expected two fetches per render across seven renders, got {fetches}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cached_poster_is_served_while_renders_are_saturated() {
    // Admission is taken inside the render path, not around the handler, so a
    // request that hits L2 does no CPU work and must not queue behind one that
    // does. Wrapping the handler would make a full cache serve at the speed of
    // its slowest render.
    let (state, _upstream) = harness(Duration::from_millis(400), Some(1)).await;

    let warm = create(&state, json!({ "tmdb_movie_id": 27205 })).await;
    let (status, _) = fetch(state.clone(), warm.clone()).await;
    assert_eq!(status, StatusCode::OK, "warm-up render failed");

    // Occupy the single render slot with a different, cold key.
    let cold = create(
        &state,
        json!({ "tmdb_movie_id": 27205, "preset": "cinematic" }),
    )
    .await;
    let blocker = tokio::spawn(fetch(state.clone(), cold));
    tokio::time::sleep(Duration::from_millis(50)).await;

    let started = std::time::Instant::now();
    let (status, cache) = fetch(state.clone(), warm).await;
    let elapsed = started.elapsed();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache, "HIT");
    assert!(
        elapsed < ADMISSION_TIMEOUT,
        "a cache hit waited {elapsed:?} behind a render"
    );

    blocker.await.expect("blocker completes");
}

#[tokio::test]
async fn admission_capacity_follows_configuration() {
    let (state, _upstream) = harness(Duration::from_millis(1), Some(3)).await;
    assert_eq!(state.admission.capacity(), 3);
    assert_eq!(state.admission.available(), 3);
}

#[tokio::test]
async fn admission_defaults_to_the_machine_size() {
    let (state, _upstream) = harness(Duration::from_millis(1), None).await;
    assert_eq!(
        state.admission.capacity(),
        Admission::for_this_machine().capacity()
    );
    assert!(state.admission.capacity() >= 1);
}

#[tokio::test]
async fn the_metrics_endpoint_reports_without_a_recorder() {
    // Tests build many states in one process and a recorder is global, so
    // state carries none. The endpoint must say so rather than panic.
    let (state, _upstream) = harness(Duration::from_millis(1), Some(1)).await;

    let response = poster_service::api::router(&state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saturating_admission_returns_503_with_retry_after() {
    // The rejection path. One render slot, several distinct cold keys so
    // single-flight cannot collapse them, and an upstream slow enough that the
    // slot stays occupied past the admission wait.
    let (state, _upstream) = harness(Duration::from_millis(600), Some(1)).await;

    let mut keys = Vec::new();
    for preset in ["standard", "cinematic", "minimal", "poster_wall"] {
        keys.push(create(&state, json!({ "tmdb_movie_id": 27205, "preset": preset })).await);
    }

    let mut tasks = Vec::new();
    for key in keys {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            let response = poster_service::api::router(&state)
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/posters/{key}.webp"))
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router responds");

            let status = response.status();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            (status, retry_after)
        }));
    }

    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.expect("request completes"));
    }

    let overloaded: Vec<_> = results
        .iter()
        .filter(|(status, _)| *status == StatusCode::SERVICE_UNAVAILABLE)
        .collect();

    assert!(
        !overloaded.is_empty(),
        "one render slot and four slow cold renders produced no rejection: {results:?}"
    );

    for (_, retry_after) in overloaded {
        assert_eq!(
            retry_after.as_deref(),
            Some("1"),
            "a 503 was returned without telling the client when to retry"
        );
    }
}
