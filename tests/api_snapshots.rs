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

/// Asserts at compile time that [`every_error`] covers the whole enum.
///
/// The fixture below was silently missing four variants added in v2 — so the
/// wire-form snapshot never covered them, and neither did the retry or cache
/// assertions. A `Vec` cannot be exhaustive on its own; this match can.
///
/// Adding a variant to `ApiError` breaks this function until it is listed
/// here, at which point the omission from `every_error` is obvious.
#[allow(dead_code)]
fn exhaustiveness_guard(error: &ApiError) {
    match error {
        ApiError::InvalidPosterPath(_)
        | ApiError::TmdbNotFound { .. }
        | ApiError::NoArtworkAvailable(_)
        | ApiError::TmdbCredentialMissing
        | ApiError::TmdbUnauthorised
        | ApiError::UnknownPreset(_)
        | ApiError::ValidationFailed(_)
        | ApiError::MalformedRequest(_)
        | ApiError::UnknownKey
        | ApiError::SourceNotFound
        | ApiError::SourceTooLarge
        | ApiError::SourceDimensionsExceeded { .. }
        | ApiError::SourceDecodeFailed(_)
        | ApiError::UpstreamUnavailable(_)
        | ApiError::UpstreamTimeout
        | ApiError::Overloaded
        | ApiError::StorageUnavailable(_)
        | ApiError::RenderFailed(_) => {}
    }
}

/// Every variant, so a new one cannot be added without appearing here.
fn every_error() -> Vec<ApiError> {
    vec![
        ApiError::InvalidPosterPath(InvalidPosterPath::MissingLeadingSlash),
        ApiError::TmdbNotFound {
            kind: "movie",
            id: 999_999_999,
        },
        ApiError::NoArtworkAvailable("movie 27205 has no poster artwork".to_owned()),
        ApiError::TmdbCredentialMissing,
        ApiError::TmdbUnauthorised,
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

/// Path to the error reference, relative to the crate root.
const ERROR_DOCS: &str = "docs/errors.md";

#[test]
fn every_code_is_documented() {
    // A code with no documentation is a code whose `type` URI points at a
    // missing anchor — the field exists precisely so an unfamiliar failure is
    // one click from an explanation, and a dangling link is worse than none.
    //
    // The summary table already listed `overloaded` when its section did not
    // exist, which is how this test came to be written.
    let docs =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ERROR_DOCS))
            .expect("the error reference is readable");

    let undocumented: Vec<_> = every_error()
        .iter()
        .map(ApiError::code)
        .filter(|code| !docs.contains(&format!("#### `{code}`")))
        .collect();

    assert!(
        undocumented.is_empty(),
        "codes with no section in {ERROR_DOCS}: {undocumented:?}"
    );
}

#[test]
fn every_documented_code_still_exists() {
    // The other direction: a section left behind after a code was renamed
    // documents something callers can never receive.
    let docs =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ERROR_DOCS))
            .expect("the error reference is readable");

    let live: Vec<_> = every_error().iter().map(ApiError::code).collect();

    let orphaned: Vec<_> = docs
        .lines()
        .filter_map(|line| line.strip_prefix("#### `")?.strip_suffix('`'))
        .filter(|code| !live.contains(code))
        .collect();

    assert!(
        orphaned.is_empty(),
        "{ERROR_DOCS} documents codes that no longer exist: {orphaned:?}"
    );
}

#[test]
fn every_error_offers_a_hint() {
    // The field is what turns "this happened" into "do this". An empty one
    // would render as a key with nothing behind it, which is worse than
    // omitting the field.
    for error in every_error() {
        let hint = error.hint();
        assert!(
            hint.len() > 20,
            "{} has no useful hint: {hint:?}",
            error.code()
        );
        assert!(
            hint.ends_with('.'),
            "{} has a hint that is not a sentence: {hint:?}",
            error.code()
        );
    }
}

#[test]
fn a_hint_does_not_merely_restate_the_detail() {
    // The two are separate so that one describes and the other prescribes.
    // A hint identical to the message would collapse that distinction.
    for error in every_error() {
        assert_ne!(
            error.hint(),
            error.to_string(),
            "{} restates its detail instead of advising",
            error.code()
        );
    }
}

#[tokio::test]
async fn the_type_uri_points_at_the_code() {
    // Constructed rather than hand-written per variant, but worth pinning:
    // a mismatch sends a reader to the wrong section, which is more confusing
    // than no link.
    for error in every_error() {
        let code = error.code();
        let response = error.into_response();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body collects")
            .to_bytes();
        let problem: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let type_uri = problem["type"].as_str().expect("type is a string");

        assert!(
            type_uri.ends_with(&format!("#{code}")),
            "{code} points at {type_uri}"
        );
        assert!(type_uri.contains("docs/errors.md"));
    }
}
