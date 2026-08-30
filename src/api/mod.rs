//! HTTP surface: routing, middleware and response shaping.
//!
//! This module is the only place that knows about axum. Everything below it
//! deals in domain types and byte buffers, which is what keeps the renderer
//! testable without a runtime.

pub mod error;
pub mod health;
pub mod posters;
pub mod presets;

use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Largest accepted request body.
///
/// A poster specification is a few hundred bytes; 16 KiB is generous for six
/// badges of 32 graphemes each and still small enough that a flood of oversized
/// bodies cannot pressure memory.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// Ceiling on how long any single request may occupy a connection.
///
/// Above the sum of the upstream timeout and a w2000 render, so it never fires
/// on legitimate work — it exists to bound a request that has gone wrong, not
/// to enforce the latency budget.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds the application router with its middleware stack.
///
/// # Arguments
///
/// * `state` — configuration, storage and the upstream client. Cloned into
///   the router; `AppState` clones are refcount bumps.
///
/// # Returns
///
/// A [`Router`] with every route mounted. The caller binds a listener.
///
/// # Examples
///
/// ```no_run
/// use poster_service::{api, config::Config, state::AppState, storage::Storage};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = Config::from_env()?;
/// let storage = config.build_storage()?;
/// let app = api::router(&AppState::new(config, storage)?);
/// # let _ = app;
/// # Ok(())
/// # }
/// ```
pub fn router(state: &AppState) -> Router {
    Router::new()
        .route("/v1/posters", post(posters::create))
        .route("/v1/posters/{key}", get(posters::get))
        .route("/v1/presets", get(presets::list))
        .with_state(state.clone())
        .merge(health::routes(state.storage.clone()))
        // A panic inside an imaging crate would otherwise take down the worker
        // rather than fail one request. tiny-skia and resvg are not written to
        // be panic-free on adversarial geometry, and the byte-level guards
        // upstream cannot rule out every shape they might reject.
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
}
