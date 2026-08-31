//! The error taxonomy and its wire representation.
//!
//! # One shape for every failure
//!
//! Every error leaves this service as an RFC 9457 `application/problem+json`
//! document, whatever went wrong and wherever it went wrong. A caller writes
//! one error handler, not one per endpoint.
//!
//! ```json
//! {
//!   "type":      "https://…/docs/errors.md#tmdb_not_found",
//!   "title":     "Not Found",
//!   "status":    404,
//!   "code":      "tmdb_not_found",
//!   "detail":    "no movie in the TMDB catalogue with id 999999999",
//!   "hint":      "Check the identifier on themoviedb.org.",
//!   "retryable": false
//! }
//! ```
//!
//! # What each field is for
//!
//! The three that carry the meaning are deliberately separate, because they
//! answer different questions and are read by different audiences:
//!
//! - **`code`** — what class of failure this is. Stable, machine-readable, and
//!   the only field a client should branch on.
//! - **`detail`** — what happened *this time*, with the specific values
//!   involved. For a human reading a log.
//! - **`hint`** — what to do about it. Also for a human, but a different human:
//!   `detail` describes, `hint` prescribes.
//!
//! Collapsing `detail` and `hint` into one string is the usual mistake. It
//! produces messages that either describe without helping, or advise without
//! saying what actually went wrong.
//!
//! `retryable` says whether repeating the identical request could ever
//! succeed. It is not a guess about load: a `404` is never retryable no matter
//! how long you wait, and a `503` always is.
//!
//! # The code is a public contract
//!
//! Clients branch on `code`, so renaming an [`ApiError`] variant is a breaking
//! change even though the type is internal. The snapshot tests exist to make
//! such a rename visible in review, and `docs/errors.md` documents every code
//! with the `type` URI pointing at it.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::render::RenderError;
use crate::spec::SpecError;
use crate::storage::StorageError;
use crate::tmdb::fetch::FetchError;
use crate::tmdb::InvalidPosterPath;

/// Every error the API can return.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The supplied path is not a valid TMDB path.
    #[error("poster path is not a valid TMDB path: {0}")]
    InvalidPosterPath(#[from] InvalidPosterPath),

    /// No entry in the TMDB catalogue under this identifier.
    #[error("no {kind} in the TMDB catalogue with id {id}")]
    TmdbNotFound {
        /// Which catalogue was searched.
        kind: &'static str,
        /// The identifier supplied.
        id: u32,
    },

    /// The catalogue entry exists but has no poster this service can use.
    #[error("{0}")]
    NoArtworkAvailable(String),

    /// No TMDB credential is configured.
    ///
    /// A deployment fault rather than a caller fault, but it is surfaced with
    /// its own code so an operator reading a log sees what to set instead of a
    /// generic internal error.
    #[error("no TMDB credential is configured; set POSTER_TMDB_API_KEY")]
    TmdbCredentialMissing,

    /// TMDB rejected the configured credential.
    #[error("TMDB rejected the configured credential")]
    TmdbUnauthorised,

    /// The named preset is not in the catalogue.
    #[error("unknown preset: {0}")]
    UnknownPreset(String),

    /// The request was well-formed but failed validation.
    #[error("{0}")]
    ValidationFailed(String),

    /// The request body could not be parsed.
    #[error("request body is malformed: {0}")]
    MalformedRequest(String),

    /// No specification is stored under this key.
    #[error("no specification stored for this key")]
    UnknownKey,

    /// Upstream reported that the artwork does not exist.
    #[error("upstream artwork not found")]
    SourceNotFound,

    /// The upstream response exceeded the byte cap.
    #[error("upstream response exceeded the byte cap")]
    SourceTooLarge,

    /// The source declared dimensions beyond the decode guard.
    #[error("source is {width}x{height}, exceeding the {max} px limit")]
    SourceDimensionsExceeded {
        /// Declared width.
        width: u32,
        /// Declared height.
        height: u32,
        /// Configured per-side maximum.
        max: u32,
    },

    /// The source could not be decoded.
    #[error("could not decode upstream artwork: {0}")]
    SourceDecodeFailed(String),

    /// Upstream was unreachable or answered unexpectedly.
    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),

    /// The upstream request timed out.
    #[error("upstream timed out")]
    UpstreamTimeout,

    /// Render capacity is exhausted.
    #[error("render capacity exhausted")]
    Overloaded,

    /// Storage could not be reached.
    #[error("storage unavailable: {0}")]
    StorageUnavailable(String),

    /// Rendering failed for a reason that is not the caller's fault.
    #[error("render failed: {0}")]
    RenderFailed(String),
}

impl ApiError {
    /// Returns the HTTP status for this error.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::InvalidPosterPath(_) | Self::UnknownPreset(_) | Self::MalformedRequest(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::ValidationFailed(_)
            | Self::SourceDimensionsExceeded { .. }
            // The entry exists, so the request is well-formed; it simply
            // cannot be satisfied, which is what 422 means.
            | Self::NoArtworkAvailable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            // A caller who named artwork that does not exist made a client
            // error about a resource. Returning 502 would invite a retry that
            // cannot succeed.
            Self::UnknownKey | Self::SourceNotFound | Self::TmdbNotFound { .. } => {
                StatusCode::NOT_FOUND
            }
            // The caller did nothing wrong and cannot fix it; a retry after
            // the operator sets the credential will work, but not sooner.
            Self::TmdbCredentialMissing | Self::TmdbUnauthorised => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            // The oversized payload is the upstream's, not the caller's. 413
            // would tell them to shrink a request body they did not send.
            Self::SourceTooLarge | Self::SourceDecodeFailed(_) | Self::UpstreamUnavailable(_) => {
                StatusCode::BAD_GATEWAY
            }
            Self::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Overloaded | Self::StorageUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::RenderFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns the stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPosterPath(_) => "invalid_poster_path",
            Self::TmdbNotFound { .. } => "tmdb_not_found",
            Self::NoArtworkAvailable(_) => "no_artwork_available",
            Self::TmdbCredentialMissing => "tmdb_credential_missing",
            Self::TmdbUnauthorised => "tmdb_unauthorised",
            Self::UnknownPreset(_) => "unknown_preset",
            Self::ValidationFailed(_) => "validation_failed",
            Self::MalformedRequest(_) => "malformed_request",
            Self::UnknownKey => "unknown_key",
            Self::SourceNotFound => "source_not_found",
            Self::SourceTooLarge => "source_too_large",
            Self::SourceDimensionsExceeded { .. } => "source_dimensions_exceeded",
            Self::SourceDecodeFailed(_) => "source_decode_failed",
            Self::UpstreamUnavailable(_) => "upstream_unavailable",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::Overloaded => "overloaded",
            Self::StorageUnavailable(_) => "storage_unavailable",
            Self::RenderFailed(_) => "render_failed",
        }
    }

    /// Returns what the caller should do about this error.
    ///
    /// Separate from the message on the variant, which says what *happened*.
    /// A caller reading `no movie in the TMDB catalogue with id 999999999`
    /// knows the fact; the hint is what turns that into a next step.
    ///
    /// Written for whoever is placed to act. Most hints address the API
    /// caller; the credential ones address whoever runs the service, since a
    /// caller cannot fix a missing environment variable.
    #[must_use]
    pub const fn hint(&self) -> &'static str {
        match self {
            Self::InvalidPosterPath(_) => {
                "Send a path from GET /v1/artwork/{kind}/{id}, or omit the field to let \
                 the service choose."
            }
            Self::TmdbNotFound { .. } => {
                "Check the identifier on themoviedb.org. Films use tmdb_movie_id and \
                 series use tmdb_tv_id; the two catalogues number separately, so a \
                 valid film id is rarely a valid series id."
            }
            Self::NoArtworkAvailable(_) => {
                "This title has no poster the service can render. Nothing to retry -- \
                 pick a different title."
            }
            Self::TmdbCredentialMissing => {
                "Set POSTER_TMDB_API_KEY on the service. Either a v3 API key or a v4 \
                 read access token works."
            }
            Self::TmdbUnauthorised => {
                "The configured POSTER_TMDB_API_KEY was rejected. Check it has not been \
                 revoked or truncated."
            }
            Self::UnknownPreset(_) => "GET /v1/presets lists the valid names.",
            Self::ValidationFailed(_) => {
                "Correct the field named in the detail. GET /v1/artwork/{kind}/{id} \
                 lists the artwork a title actually offers."
            }
            Self::MalformedRequest(_) => {
                "Check the JSON parses and that every field is spelled as documented; \
                 unknown fields are rejected rather than ignored."
            }
            Self::UnknownKey => {
                "Keys come from POST /v1/posters. Post the specification again to get a \
                 fresh one -- an identical specification returns an identical key."
            }
            Self::SourceNotFound => {
                "The artwork this poster was built from is no longer on the TMDB CDN. \
                 Post the request again to resolve current artwork."
            }
            Self::SourceTooLarge | Self::SourceDimensionsExceeded { .. } => {
                "The upstream artwork is beyond the size this service will decode. \
                 Choose different artwork from GET /v1/artwork/{kind}/{id}."
            }
            Self::SourceDecodeFailed(_) => {
                "The upstream artwork could not be decoded. Choose different artwork \
                 from GET /v1/artwork/{kind}/{id}."
            }
            Self::UpstreamUnavailable(_) | Self::UpstreamTimeout => {
                "TMDB is not responding. Retry after the interval in Retry-After."
            }
            Self::Overloaded => {
                "The service is at render capacity. Retry after the interval in \
                 Retry-After; sustained rejections mean it needs more replicas."
            }
            Self::StorageUnavailable(_) => {
                "The service cannot reach its object storage. Retry after the interval \
                 in Retry-After, and check /readyz."
            }
            Self::RenderFailed(_) => {
                "The service failed to produce this poster. Not a fault in the request; \
                 report it with the x-request-id from the response headers."
            }
        }
    }

    /// Returns how long a client should wait before retrying, in seconds.
    ///
    /// `None` means the error is not retryable and a retry will fail the same
    /// way. Distinguishing the two is the difference between a client backing
    /// off and a client hammering an endpoint that can never succeed.
    #[must_use]
    pub const fn retry_after(&self) -> Option<u32> {
        match self {
            // One second: the semaphore drains in tens of milliseconds, so a
            // longer wait would idle a client that could already be served.
            Self::Overloaded | Self::UpstreamTimeout | Self::UpstreamUnavailable(_) => Some(1),
            // Two: a storage blip takes longer to clear than a local queue,
            // and hammering it makes recovery slower.
            Self::StorageUnavailable(_) => Some(2),
            _ => None,
        }
    }

    /// Returns the `Cache-Control` value for this error.
    ///
    /// Errors are `no-store` with one exception: an unknown key is unknown
    /// permanently, so letting the CDN absorb the retry storm that follows a
    /// bad link being shared is worth a minute of caching.
    #[must_use]
    pub const fn cache_control(&self) -> &'static str {
        match self {
            Self::UnknownKey => "public, max-age=60",
            _ => "no-store",
        }
    }
}

/// Where each error code is documented.
///
/// Forms the RFC 9457 `type` URI. A reader who does not recognise a code has
/// somewhere to go, which is the whole reason the field exists.
const DOCS_BASE: &str = "https://github.com/ibrahimadlani/ibrahim-posters/blob/main/docs/errors.md";

/// An RFC 9457 problem document.
///
/// Field order matches the order a human reads them in: what kind of problem,
/// then what happened, then what to do.
#[derive(Debug, Serialize)]
pub struct Problem {
    /// URI identifying this error class. Points at the documentation for the
    /// code, so an unfamiliar failure is one click from an explanation.
    #[serde(rename = "type")]
    pub type_uri: String,
    /// Short summary of the *class* of error. The same for every occurrence.
    pub title: &'static str,
    /// HTTP status, repeated in the body as the RFC prescribes.
    pub status: u16,
    /// Stable machine-readable code. The only field a client should branch on.
    pub code: &'static str,
    /// What happened this time, with the values involved.
    pub detail: String,
    /// What to do about it.
    pub hint: &'static str,
    /// Whether repeating the identical request could ever succeed.
    pub retryable: bool,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let retry_after = self.retry_after();

        // 5xx is the service's problem and is logged at error; 4xx is the
        // caller's and is logged at debug, or a scan for invalid paths would
        // fill the error log with entries nobody should act on.
        if status.is_server_error() {
            tracing::error!(code = self.code(), error = %self, "request failed");
        } else {
            tracing::debug!(code = self.code(), error = %self, "request rejected");
        }

        let problem = Problem {
            type_uri: format!("{DOCS_BASE}#{}", self.code()),
            title: title_for(status),
            status: status.as_u16(),
            code: self.code(),
            detail: self.to_string(),
            hint: self.hint(),
            retryable: retry_after.is_some(),
        };

        let mut response = (status, Json(problem)).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static(self.cache_control()),
        );
        if let Some(seconds) = retry_after {
            if let Ok(value) = header::HeaderValue::from_str(&seconds.to_string()) {
                headers.insert(header::RETRY_AFTER, value);
            }
        }

        response
    }
}

/// Returns the RFC 9457 `title` for a status.
const fn title_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "Bad Request",
        StatusCode::NOT_FOUND => "Not Found",
        StatusCode::UNPROCESSABLE_ENTITY => "Unprocessable Entity",
        StatusCode::BAD_GATEWAY => "Bad Gateway",
        StatusCode::SERVICE_UNAVAILABLE => "Service Unavailable",
        StatusCode::GATEWAY_TIMEOUT => "Gateway Timeout",
        _ => "Internal Server Error",
    }
}

impl From<SpecError> for ApiError {
    fn from(error: SpecError) -> Self {
        match error {
            SpecError::UnknownPreset(name) => Self::UnknownPreset(name),
            // The title exists but offers nothing renderable, which is a fact
            // about the catalogue rather than about the request.
            SpecError::NoArtwork(detail) => Self::NoArtworkAvailable(detail),
            other => Self::ValidationFailed(other.to_string()),
        }
    }
}

impl From<FetchError> for ApiError {
    fn from(error: FetchError) -> Self {
        match error {
            FetchError::NotFound => Self::SourceNotFound,
            FetchError::Timeout => Self::UpstreamTimeout,
            FetchError::TooLarge { .. } => Self::SourceTooLarge,
            FetchError::DimensionsExceeded { width, height, max } => {
                Self::SourceDimensionsExceeded { width, height, max }
            }
            FetchError::UnrecognisedFormat => {
                Self::SourceDecodeFailed("not a recognised image".to_owned())
            }
            FetchError::UnexpectedStatus { .. } | FetchError::Transport(_) => {
                Self::UpstreamUnavailable(error.to_string())
            }
        }
    }
}

impl From<crate::tmdb::api::MetadataError> for ApiError {
    fn from(error: crate::tmdb::api::MetadataError) -> Self {
        use crate::tmdb::api::MetadataError;
        match error {
            MetadataError::NotFound { kind, id } => Self::TmdbNotFound {
                kind: kind.segment(),
                id,
            },
            MetadataError::NoPoster { .. } => Self::NoArtworkAvailable(error.to_string()),
            MetadataError::Unauthorised => Self::TmdbUnauthorised,
            // Rate limiting is the one metadata failure a client should retry:
            // it clears on its own, where the others need a different request
            // or a different configuration.
            MetadataError::RateLimited => Self::Overloaded,
            MetadataError::UnexpectedStatus { .. }
            | MetadataError::Transport(_)
            | MetadataError::Malformed(_) => Self::UpstreamUnavailable(error.to_string()),
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            // A stored specification that will not parse or is out of range is
            // this service's problem, not a transient one: retrying reads the
            // same bytes and fails identically.
            StorageError::MalformedSpec { .. } | StorageError::InvalidSpec { .. } => {
                Self::RenderFailed(error.to_string())
            }
            StorageError::Backend(_) => Self::StorageUnavailable(error.to_string()),
        }
    }
}

impl From<RenderError> for ApiError {
    fn from(error: RenderError) -> Self {
        match error {
            // A decode failure is the upstream's bytes being unusable, which
            // is a gateway problem rather than an internal one.
            RenderError::Decode(detail) => Self::SourceDecodeFailed(detail),
            other => Self::RenderFailed(other.to_string()),
        }
    }
}
