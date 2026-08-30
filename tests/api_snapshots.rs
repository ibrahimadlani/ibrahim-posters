//! Snapshots of the API's public contract.
//!
//! The `code` field of an error body is a stable contract: clients branch on
//! it, so renaming an `ApiError` variant is a breaking change even though the
//! type is internal. These snapshots make such a rename visible in review
//! rather than discoverable in production.

use axum::response::IntoResponse;
use http_body_util::BodyExt as _;
use insta::{assert_json_snapshot, assert_snapshot};
use poster_service::api::error::ApiError;
use poster_service::tmdb::InvalidPosterPath;

/// Renders one error to its wire form: status, headers, body.
async fn render(error: ApiError) -> String {
    let code = error.code();
    let response = error.into_response();
    let status = response.status();

    let headers = response.headers().clone();
    let cache_control = headers
        .get(axum::http::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("(none)")
        .to_owned();
    let retry_after = headers
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("(none)")
        .to_owned();
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("(none)")
        .to_owned();

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    let body = String::from_utf8_lossy(&bytes);

    format!(
        "code:          {code}\nstatus:        {status}\ncontent-type:  {content_type}\ncache-control: {cache_control}\nretry-after:   {retry_after}\nbody:          {body}"
    )
}

/// Every variant, so a new one cannot be added without appearing here.
fn every_error() -> Vec<ApiError> {
    vec![
        ApiError::InvalidPosterPath(InvalidPosterPath::MissingLeadingSlash),
        ApiError::UnknownPreset("nope".to_owned()),
        ApiError::ValidationFailed("at most 6 badges are allowed, found 12".to_owned()),
        ApiError::MalformedRequest("expected value at line 1 column 3".to_owned()),
        ApiError::UnknownKey,
        ApiError::SourceNotFound,
        ApiError::SourceTooLarge,
        ApiError::SourceDimensionsExceeded {
            width: 60_000,
            height: 60_000,
            max: 8000,
        },
        ApiError::SourceDecodeFailed("not a recognised image".to_owned()),
        ApiError::UpstreamUnavailable("upstream responded 503 Service Unavailable".to_owned()),
        ApiError::UpstreamTimeout,
        ApiError::Overloaded,
        ApiError::StorageUnavailable("connection refused".to_owned()),
        ApiError::RenderFailed("could not encode output".to_owned()),
    ]
}

#[tokio::test]
async fn every_error_variant_has_a_pinned_wire_form() {
    let mut rendered = Vec::new();
    for error in every_error() {
        rendered.push(render(error).await);
    }
    assert_snapshot!(rendered.join("\n\n---\n\n"));
}

#[test]
fn the_status_and_code_table_is_pinned() {
    // The compact view of PLAN.md section 8, in one place a reviewer can scan.
    let table: Vec<_> = every_error()
        .into_iter()
        .map(|error| {
            serde_json::json!({
                "code": error.code(),
                "status": error.status().as_u16(),
                "retry_after": error.retry_after(),
                "cache_control": error.cache_control(),
            })
        })
        .collect();
    assert_json_snapshot!(table);
}

#[test]
fn every_code_is_unique() {
    // Two variants sharing a code would make them indistinguishable to a
    // client branching on it.
    let mut codes: Vec<_> = every_error().iter().map(ApiError::code).collect();
    codes.sort_unstable();
    let count = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), count, "duplicate error codes: {codes:?}");
}

#[test]
fn only_transient_errors_are_retryable() {
    // A retryable code invites a client to try again. Marking a permanent
    // failure retryable turns one bad request into a loop.
    for error in every_error() {
        let retryable = error.retry_after().is_some();
        let expected = matches!(
            error,
            ApiError::Overloaded
                | ApiError::StorageUnavailable(_)
                | ApiError::UpstreamTimeout
                | ApiError::UpstreamUnavailable(_)
        );
        assert_eq!(
            retryable,
            expected,
            "{} has the wrong retry disposition",
            error.code()
        );
    }
}

#[test]
fn only_an_unknown_key_is_cacheable() {
    for error in every_error() {
        let cacheable = error.cache_control() != "no-store";
        assert_eq!(
            cacheable,
            matches!(error, ApiError::UnknownKey),
            "{} has the wrong cache disposition",
            error.code()
        );
    }
}
