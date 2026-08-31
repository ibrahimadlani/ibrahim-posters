//! HTTP surface: routing, middleware and response shaping.
//!
//! This module is the only place that knows about axum. Everything below it
//! deals in domain types and byte buffers, which is what keeps the renderer
//! testable without a runtime.

pub mod artwork;
pub mod error;
pub mod health;
pub mod metrics;
pub mod posters;
pub mod presets;

use std::time::Duration;

use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
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
        .route("/v1/artwork/{kind}/{id}", get(artwork::list))
        .route("/v1/presets", get(presets::list))
        .route("/metrics", get(metrics::scrape))
        .with_state(state.clone())
        .merge(health::routes(state.storage.clone()))
        // A panic inside an imaging crate would otherwise take down the worker
        // rather than fail one request. tiny-skia and resvg are not written to
        // be panic-free on adversarial geometry, and the byte-level guards
        // upstream cannot rule out every shape they might reject.
        .layer(axum::middleware::from_fn(record_request))
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
}

/// Times every request and records it against the *matched route*.
///
/// The route template is used as the label, not the request path. A poster URL
/// contains a 64-character key, so labelling by path would create one metric
/// series per poster ever requested — the standard way to make a Prometheus
/// server run out of memory. `MatchedPath` gives `/v1/posters/{key}`, which is
/// bounded by the number of routes.
///
/// Implemented as middleware rather than in each handler so a new endpoint is
/// instrumented by existing, and so the timing covers response construction
/// rather than stopping at the handler's return.
async fn record_request(request: Request, next: Next) -> Response {
    let endpoint = request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED, |matched| label_for(matched.as_str()));

    let started = std::time::Instant::now();
    let response = next.run(request).await;

    crate::observability::record_request(endpoint, response.status().as_u16(), started.elapsed());

    response
}

/// Label used for requests that matched no route.
const UNMATCHED: &str = "unmatched";

/// Maps a matched route template to a `'static` label.
///
/// Written as a match over the route table rather than by interning the
/// matched string. The metrics macros want a `&'static str`, and the obvious
/// way to produce one — `Box::leak` — allocates on *every request*, not once
/// per distinct route. Twenty bytes per request is a slow leak that never
/// stops growing, which is a poor trade for avoiding a match arm.
///
/// Adding a route without adding an arm here reports it as `other` rather than
/// silently creating an unbounded label, which is the safe direction.
fn label_for(matched: &str) -> &'static str {
    match matched {
        "/v1/posters" => "/v1/posters",
        "/v1/artwork/{kind}/{id}" => "/v1/artwork/{kind}/{id}",
        "/v1/posters/{key}" => "/v1/posters/{key}",
        "/v1/presets" => "/v1/presets",
        "/healthz" => "/healthz",
        "/readyz" => "/readyz",
        "/metrics" => "/metrics",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every route the router mounts.
    const ROUTES: &[&str] = &[
        "/v1/posters",
        "/v1/artwork/{kind}/{id}",
        "/v1/posters/{key}",
        "/v1/presets",
        "/healthz",
        "/readyz",
        "/metrics",
    ];

    #[test]
    fn every_route_has_its_own_label() {
        // A route missing an arm falls through to "other", which merges its
        // latency with everything else and makes the metric useless for it.
        for route in ROUTES {
            assert_eq!(
                label_for(route),
                *route,
                "{route} is not labelled as itself"
            );
        }
    }

    #[test]
    fn an_unknown_route_collapses_rather_than_creating_a_label() {
        // The property that keeps cardinality bounded: a path containing a
        // 64-character key must never become its own metric series.
        let key = "a".repeat(64);
        assert_eq!(label_for(&format!("/v1/posters/{key}.webp")), "other");
        assert_eq!(label_for("/anything/else"), "other");
    }

    #[test]
    fn the_label_set_is_small_and_fixed() {
        let mut labels: Vec<_> = ROUTES.iter().map(|route| label_for(route)).collect();
        labels.push(UNMATCHED);
        labels.push(label_for("/unknown"));

        let total = labels.len();
        labels.sort_unstable();
        labels.dedup();

        assert_eq!(labels.len(), total, "two routes share a label");
        assert!(labels.len() < 16, "the label set is larger than expected");
    }
}
