//! Health endpoint round trip.
//!
//! Drives the router through `oneshot` rather than binding a socket: no port
//! allocation, no flakiness from a port already in use, and the test exercises
//! the same router that `main` serves.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// Builds application state around a storage backend.
fn state(storage: poster_service::storage::Storage) -> poster_service::state::AppState {
    poster_service::state::AppState::new(poster_service::config::Config::defaults(), storage)
        .expect("state builds")
}

#[tokio::test]
async fn healthz_reports_ok() {
    let response =
        poster_service::api::router(&state(poster_service::storage::Storage::in_memory()))
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

    assert_eq!(response.status(), StatusCode::OK);

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn readyz_reports_ready() {
    let response =
        poster_service::api::router(&state(poster_service::storage::Storage::in_memory()))
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    let response =
        poster_service::api::router(&state(poster_service::storage::Storage::in_memory()))
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn readyz_reports_unready_when_storage_is_unreachable() {
    // A LocalFileSystem rooted at a regular file rather than a directory:
    // it constructs cleanly and fails on every operation, which is a real
    // backend failing for a real reason rather than a hand-written stub whose
    // behaviour is whatever the test author decided.
    use object_store::local::LocalFileSystem;
    use std::sync::Arc;

    let not_a_directory =
        std::env::temp_dir().join(format!("poster-service-readyz-{}", std::process::id()));
    std::fs::write(&not_a_directory, b"not a directory").expect("fixture writes");

    let backend = LocalFileSystem::new_with_prefix(&not_a_directory).expect("constructs");
    let storage = poster_service::storage::Storage::new(Arc::new(backend));

    let response = poster_service::api::router(&state(storage))
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    assert_eq!(&body[..], b"storage unreachable");

    std::fs::remove_file(&not_a_directory).expect("fixture cleans up");
}

#[tokio::test]
async fn healthz_stays_green_when_storage_is_unreachable() {
    // The distinction the two probes exist for. Restarting a healthy process
    // because its object store is briefly unavailable removes capacity at
    // exactly the moment the system is already degraded.
    use object_store::local::LocalFileSystem;
    use std::sync::Arc;

    let not_a_directory =
        std::env::temp_dir().join(format!("poster-service-healthz-{}", std::process::id()));
    std::fs::write(&not_a_directory, b"not a directory").expect("fixture writes");

    let backend = LocalFileSystem::new_with_prefix(&not_a_directory).expect("constructs");
    let storage = poster_service::storage::Storage::new(Arc::new(backend));

    let response = poster_service::api::router(&state(storage))
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "liveness must not depend on a dependency the process does not own"
    );

    std::fs::remove_file(&not_a_directory).expect("fixture cleans up");
}
