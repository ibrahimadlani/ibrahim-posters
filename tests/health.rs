//! Health endpoint round trip.
//!
//! Drives the router through `oneshot` rather than binding a socket: no port
//! allocation, no flakiness from a port already in use, and the test exercises
//! the same router that `main` serves.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

#[tokio::test]
async fn healthz_reports_ok() {
    let response = poster_service::api::router()
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
    let response = poster_service::api::router()
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
    let response = poster_service::api::router()
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
