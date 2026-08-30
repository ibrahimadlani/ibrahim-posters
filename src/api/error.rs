//! The error taxonomy and its wire representation.
//!
//! Bodies follow RFC 9457 (`application/problem+json`). The `code` field is a
//! **stable part of the public contract**: clients branch on it, so renaming a
//! variant is a breaking change even though this type is internal. The
//! snapshot tests exist to make such a rename visible in review.

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
            Self::ValidationFailed(_) | Self::SourceDimensionsExceeded { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            // A caller who named artwork that does not exist made a client
            // error about a resource. Returning 502 would invite a retry that
            // cannot succeed.
            Self::UnknownKey | Self::SourceNotFound => StatusCode::NOT_FOUND,
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

/// An RFC 9457 problem document.
#[derive(Debug, Serialize)]
pub struct Problem {
    /// Short, stable, human-readable summary of the error type.
    pub title: &'static str,
    /// HTTP status code, repeated in the body as the RFC prescribes.
    pub status: u16,
    /// Stable machine-readable code. Clients branch on this.
    pub code: &'static str,
    /// Human-readable explanation of this particular occurrence.
    pub detail: String,
    /// Whether retrying the identical request could succeed.
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
            title: title_for(status),
            status: status.as_u16(),
            code: self.code(),
            detail: self.to_string(),
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
