//! Poster creation and retrieval.
//!
//! # Why two endpoints
//!
//! A CDN will not cache a `POST`, and at a target hit rate above 90 % the CDN
//! is doing most of the work. So creation and retrieval are split: `POST` is
//! cheap — validate, resolve, hash, one small write, no image work — and
//! returns an opaque key; `GET` carries that key and is immutably cacheable.
//!
//! See `docs/adr/0002-post-then-get-split-over-signed-urls.md`.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::render::{self, source};
use crate::spec::{preset, CacheKey, OutputWidth, PosterRequest, ResolvedSpec};
use crate::state::AppState;
use crate::storage::Storage;
use crate::tmdb::{fetch, PosterPath, TmdbSize};

/// Parses and validates a request body.
///
/// Deserialisation is attempted directly and only re-examined on failure, so
/// the happy path pays for one parse.
///
/// The re-examination exists because `PosterPath` validates inside its
/// `Deserialize`, which means a bad path arrives as a generic serde error
/// indistinguishable from a misspelled field. Both would report
/// `malformed_request`, and `invalid_poster_path` — a code the API contract
/// promises and clients branch on — would never be reachable. Checking the
/// path fields explicitly on the error path restores the distinction without
/// duplicating the request type or costing anything when the body is valid.
fn parse_request(body: &[u8]) -> Result<PosterRequest, ApiError> {
    match serde_json::from_slice::<PosterRequest>(body) {
        Ok(request) => Ok(request),
        Err(error) => {
            // Re-parse loosely to tell "the path is wrong" apart from "the
            // body is wrong". A body that will not parse as JSON at all is
            // neither, and falls through to the original error.
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
                for field in ["source", "logo"] {
                    if let Some(candidate) = value.get(field).and_then(serde_json::Value::as_str) {
                        if let Err(invalid) = PosterPath::parse(candidate) {
                            return Err(ApiError::InvalidPosterPath(invalid));
                        }
                    }
                }
            }
            Err(ApiError::MalformedRequest(error.to_string()))
        }
    }
}

/// Response to a successful `POST /v1/posters`.
#[derive(Debug, Serialize)]
pub struct Created {
    /// Content-addressed key, 64 lowercase hex characters.
    pub key: String,
    /// Where to fetch the rendered poster.
    pub url: String,
}

/// Validates a request, persists its resolved specification, and returns the
/// key it is addressed by.
///
/// Does no image work: this handler performs one small write and nothing else,
/// which is what keeps it cheap enough to sit outside the cache.
///
/// Posting the same specification twice returns the same key and overwrites an
/// identical object, so the operation is idempotent in effect even though it
/// is not in the HTTP sense.
///
/// # Errors
///
/// [`ApiError::UnknownPreset`] or [`ApiError::ValidationFailed`] if the
/// request cannot be resolved, and [`ApiError::StorageUnavailable`] if the
/// specification cannot be written.
pub async fn create(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let request = parse_request(&body)?;

    let spec = preset::resolve(&request)?;
    let key = state.storage.put_spec(&spec).await?;

    Ok((
        StatusCode::CREATED,
        Json(Created {
            key: key.to_hex(),
            url: state.config.poster_url(&key.to_hex()),
        }),
    ))
}

/// Renders or serves a poster by key.
///
/// The response is immutably cacheable, which is only honest because the key
/// is a hash of the resolved specification *including* `RENDER_VERSION`: a
/// renderer change cannot be served from a stale entry because it cannot
/// produce the same key.
///
/// # Errors
///
/// [`ApiError::UnknownKey`] if nothing was posted under this key, plus any
/// upstream, storage or render failure.
pub async fn get(
    State(state): State<AppState>,
    Path(key_with_extension): Path<String>,
) -> Result<Response, ApiError> {
    let key = key_with_extension
        .strip_suffix(".webp")
        .and_then(CacheKey::from_hex)
        .ok_or(ApiError::UnknownKey)?;

    // L2 hit: the common path once a poster has been requested even once.
    if let Some(bytes) = state.storage.get_l2_poster(key).await? {
        return Ok(poster_response(bytes.to_vec(), CacheStatus::Hit));
    }

    let spec = state
        .storage
        .get_spec(key)
        .await?
        .ok_or(ApiError::UnknownKey)?;

    let assets = gather_assets(&state, &spec).await?;

    // Rendering is CPU-bound and synchronous. Running it on a tokio worker
    // would block that worker for the whole render, starving every other
    // connection the runtime is multiplexing onto the same thread.
    let rendered = tokio::task::spawn_blocking(move || render::render_encoded(&spec, &assets))
        .await
        .map_err(|error| ApiError::RenderFailed(format!("render task failed: {error}")))??;

    // Written before responding so a concurrent request for the same key finds
    // it. Failing to cache is not worth failing the request over, so a write
    // error is logged and the poster is still served.
    if let Err(error) = state.storage.put_l2_poster(key, rendered.clone()).await {
        tracing::warn!(%error, key = %key, "rendered poster could not be cached");
    }

    Ok(poster_response(rendered, CacheStatus::Miss))
}

/// Whether a response came from the cache.
#[derive(Debug, Clone, Copy)]
enum CacheStatus {
    /// Served from the L2 tier.
    Hit,
    /// Rendered for this request.
    Miss,
}

impl CacheStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "HIT",
            Self::Miss => "MISS",
        }
    }
}

/// Builds the poster response with its caching headers.
fn poster_response(bytes: Vec<u8>, status: CacheStatus) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/webp"),
            // One year and immutable. Honest only because the key encodes
            // RENDER_VERSION; see the handler documentation.
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::HeaderName::from_static("x-cache"), status.as_str()),
        ],
        Body::from(bytes),
    )
        .into_response()
}

/// Fetches the artwork a specification refers to, using the L1 tier.
async fn gather_assets(state: &AppState, spec: &ResolvedSpec) -> Result<render::Assets, ApiError> {
    let background = load_background(state, &spec.source, spec.width).await?;

    let logo = match &spec.logo {
        Some(path) => Some(load_logo(state, path).await?),
        None => None,
    };

    Ok(render::Assets { background, logo })
}

/// Returns background artwork resized to fill the output frame, via L1.
///
/// L1 stores the artwork already resized, so a repeat request for the same
/// source at the same width skips both the upstream fetch and the resize —
/// which measurement puts at 12.4 ms, the second most expensive stage.
///
/// **Backgrounds only.** A logo must not come through here: resizing to fill
/// the poster frame crops away its aspect ratio, and the renderer's placement
/// logic then fits whatever survived. See [`load_logo`].
async fn load_background(
    state: &AppState,
    path: &PosterPath,
    width: OutputWidth,
) -> Result<Vec<u8>, ApiError> {
    if let Some(cached) = state.storage.get_l1_source(path, width).await? {
        return Ok(cached.to_vec());
    }

    let url = path.cdn_url(&state.config.tmdb_image_base, upstream_size(width));
    let fetched = fetch::fetch_image(&state.http, &url, state.config.fetch_limits()).await?;

    let (target_width, target_height) = width.dimensions();
    let quality = state.config.intermediate_quality();

    // Decode, resize and re-encode are CPU-bound; the same reasoning as the
    // render itself applies.
    let resized = tokio::task::spawn_blocking(move || {
        let decoded = source::decode(&fetched.bytes)?;
        let fitted = source::resize_to_fill(&decoded, target_width, target_height)?;
        render::encode::webp(&fitted, quality)
    })
    .await
    .map_err(|error| ApiError::RenderFailed(format!("resize task failed: {error}")))??;

    if let Err(error) = state
        .storage
        .put_l1_source(path, width, resized.clone())
        .await
    {
        tracing::warn!(%error, "resized source could not be cached");
    }

    Ok(resized)
}

/// Returns a title logo at its natural size, via the L1 logo tier.
///
/// Deliberately does *not* resize. The renderer derives a placement from the
/// logo's intrinsic aspect ratio and fits it there, so pre-scaling to the
/// poster frame would destroy the input that placement depends on — the logo
/// would arrive already cropped to 2:3 and be fitted a second time.
///
/// The bytes are cached exactly as fetched rather than re-encoded. Logos carry
/// alpha and hard edges, which a lossy pass degrades visibly where a
/// photographic background tolerates it.
async fn load_logo(state: &AppState, path: &PosterPath) -> Result<Vec<u8>, ApiError> {
    if let Some(cached) = state.storage.get_l1_logo(path).await? {
        return Ok(cached.to_vec());
    }

    // Requested at the larger size regardless of output width: a logo is
    // scaled down to its placement, and starting from more pixels costs one
    // resample rather than an upscale.
    let url = path.cdn_url(&state.config.tmdb_image_base, TmdbSize::W1280);
    let fetched = fetch::fetch_image(&state.http, &url, state.config.fetch_limits()).await?;

    if let Err(error) = state.storage.put_l1_logo(path, fetched.bytes.clone()).await {
        tracing::warn!(%error, "logo could not be cached");
    }

    Ok(fetched.bytes)
}

/// Returns the TMDB size to request for a given output width.
///
/// One step above the output width rather than `original`: TMDB originals are
/// occasionally very large, and the byte cap would reject them only after the
/// transfer had been paid for.
const fn upstream_size(width: OutputWidth) -> TmdbSize {
    match width {
        OutputWidth::W1000 => TmdbSize::W780,
        OutputWidth::W2000 => TmdbSize::W1280,
    }
}

/// Reports whether storage is reachable, for the readiness probe.
///
/// # Errors
///
/// [`ApiError::StorageUnavailable`] if the backend cannot be reached.
pub async fn check_storage(storage: &Storage) -> Result<(), ApiError> {
    storage.check_reachable().await.map_err(Into::into)
}
