//! Byte-capped streaming fetch of upstream artwork.
//!
//! # Threat model
//!
//! The upstream is a CDN the operator configured, but its *response* is not
//! trusted. Three guards apply, and each covers something the others do not:
//!
//! 1. **A total timeout**, so a slow-loris response cannot pin a task forever.
//! 2. **A cap on bytes actually read.** Under chunked transfer encoding there
//!    is no `Content-Length` at all, so a cap that consulted the header would
//!    have nothing to check and would read until memory ran out. The streamed
//!    count is the authoritative one; the header is only ever an optimisation.
//! 3. **A dimension guard applied to the header**, because compressed size
//!    bounds nothing: a 40 KB JPEG can declare a decode target of several
//!    gigabytes.
//!
//! The dimension guard runs *during* the stream rather than after it. As soon
//! as enough bytes have arrived to read a header, the declared size is checked
//! and an oversized image is abandoned mid-transfer. Checking after the body
//! completed would mean paying for the whole download in order to learn it was
//! never going to be used.

use std::time::Duration;

use reqwest::{Client, StatusCode};

use crate::tmdb::probe::{self, Dimensions, MAX_HEADER_BYTES};

/// Why an upstream fetch failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// Upstream reported that the artwork does not exist.
    #[error("upstream artwork not found")]
    NotFound,
    /// Upstream answered with an unexpected status.
    #[error("upstream responded {status}")]
    UnexpectedStatus {
        /// The status returned.
        status: StatusCode,
    },
    /// The request exceeded its total timeout.
    #[error("upstream timed out")]
    Timeout,
    /// The connection failed or was interrupted.
    #[error("upstream transport error: {0}")]
    Transport(String),
    /// The response exceeded the configured byte cap.
    #[error("upstream response exceeded the {cap} byte cap")]
    TooLarge {
        /// The cap that was exceeded.
        cap: usize,
    },
    /// The declared dimensions exceeded the guard.
    #[error("source is {width}x{height}, exceeding the {max} px limit")]
    DimensionsExceeded {
        /// Declared width.
        width: u32,
        /// Declared height.
        height: u32,
        /// Configured per-side maximum.
        max: u32,
    },
    /// The response was not a recognised image container.
    #[error("upstream response is not a recognised image")]
    UnrecognisedFormat,
}

/// Limits applied to every upstream fetch.
#[derive(Debug, Clone, Copy)]
pub struct FetchLimits {
    /// Largest response body accepted, in bytes.
    pub max_bytes: usize,
    /// Largest accepted dimension, per side, in pixels.
    pub max_dimension: u32,
    /// Total time allowed for the request.
    pub timeout: Duration,
}

impl Default for FetchLimits {
    /// Returns the limits from `PLAN.md` section 9.
    ///
    /// 20 MB comfortably exceeds any TMDB original while staying far below
    /// anything that would pressure memory at the configured concurrency.
    /// 8000 px per side is roughly four times the largest artwork TMDB
    /// serves, so it rejects bombs without rejecting real inputs.
    fn default() -> Self {
        Self {
            max_bytes: 20 * 1024 * 1024,
            max_dimension: 8000,
            timeout: Duration::from_secs(3),
        }
    }
}

/// Artwork fetched from upstream, with its declared dimensions.
#[derive(Debug, Clone)]
pub struct FetchedImage {
    /// The complete response body.
    pub bytes: Vec<u8>,
    /// Dimensions read from the header, already checked against the guard.
    pub dimensions: Dimensions,
}

/// Builds the HTTP client used for every upstream fetch.
///
/// # Arguments
///
/// * `limits` — supplies the total request timeout.
///
/// # Returns
///
/// A configured [`Client`].
///
/// # Errors
///
/// Returns the underlying `reqwest` error if the TLS backend cannot be
/// initialised.
///
/// # Panics
///
/// Does not panic.
pub fn client(limits: FetchLimits) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(limits.timeout)
        // Redirects are refused rather than followed. The path grammar
        // guarantees the *first* request goes to the configured host; without
        // this, a compromised or misconfigured CDN could redirect the fetch to
        // an internal address and defeat that guarantee entirely.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("poster-service/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Fetches artwork, enforcing every limit in `limits`.
///
/// # Arguments
///
/// * `client` — a client built by [`client`].
/// * `url` — an absolute URL produced by [`crate::tmdb::PosterPath::cdn_url`].
///   Never a caller-supplied string.
/// * `limits` — the byte, dimension and time limits to enforce.
///
/// # Returns
///
/// The response body together with the dimensions read from its header.
///
/// # Errors
///
/// See [`FetchError`]. Note that [`FetchError::DimensionsExceeded`] and
/// [`FetchError::TooLarge`] may be returned before the body has finished
/// arriving; the transfer is abandoned at that point.
pub async fn fetch_image(
    client: &Client,
    url: &str,
    limits: FetchLimits,
) -> Result<FetchedImage, FetchError> {
    let mut response = client.get(url).send().await.map_err(|e| classify(&e))?;

    match response.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => return Err(FetchError::NotFound),
        status => return Err(FetchError::UnexpectedStatus { status }),
    }

    // Acting on a declared length that already exceeds the cap saves the
    // transfer entirely. It is only ever an optimisation: a chunked response
    // declares no length, and the streamed count below is what actually
    // enforces the limit.
    if let Some(declared) = response.content_length() {
        if declared > limits.max_bytes as u64 {
            return Err(FetchError::TooLarge {
                cap: limits.max_bytes,
            });
        }
    }

    let mut body: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut dimensions: Option<Dimensions> = None;

    while let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? {
        if body.len() + chunk.len() > limits.max_bytes {
            return Err(FetchError::TooLarge {
                cap: limits.max_bytes,
            });
        }
        body.extend_from_slice(&chunk);

        // Probe as soon as a header could be complete, so an oversized image
        // is abandoned mid-transfer rather than after it.
        if dimensions.is_none() {
            if let Some(probed) = probe::probe(&body) {
                check_dimensions(probed, limits)?;
                dimensions = Some(probed);
            } else if body.len() > MAX_HEADER_BYTES {
                // Past this point the header is not merely incomplete; the
                // response is not an image this service can use, and reading
                // the rest would be wasted transfer.
                return Err(FetchError::UnrecognisedFormat);
            }
        }
    }

    let dimensions = dimensions.ok_or(FetchError::UnrecognisedFormat)?;

    Ok(FetchedImage {
        bytes: body,
        dimensions,
    })
}

/// Rejects dimensions above the guard.
fn check_dimensions(dimensions: Dimensions, limits: FetchLimits) -> Result<(), FetchError> {
    if dimensions.width > limits.max_dimension || dimensions.height > limits.max_dimension {
        return Err(FetchError::DimensionsExceeded {
            width: dimensions.width,
            height: dimensions.height,
            max: limits.max_dimension,
        });
    }
    Ok(())
}

/// Maps a `reqwest` error onto the taxonomy.
///
/// Timeouts are separated from other transport failures because they carry a
/// different retry story: a timeout is worth retrying, a connection refused
/// against a CDN generally is not.
fn classify(error: &reqwest::Error) -> FetchError {
    if error.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Transport(error.to_string())
    }
}
