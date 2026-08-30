//! Liveness and readiness probes.
//!
//! The two are deliberately distinct. Liveness answers "is this process
//! wedged, should the orchestrator restart it"; readiness answers "can this
//! process serve traffic right now". Conflating them causes an orchestrator to
//! restart a healthy process because a dependency it does not own is briefly
//! unavailable — which removes capacity at precisely the moment the system is
//! already degraded.

use axum::{routing::get, Router};

/// Mounts the health routes.
///
/// # Returns
///
/// A [`Router`] exposing `GET /healthz` and `GET /readyz`.
pub fn routes() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
}

/// Liveness probe.
///
/// Returns `200 OK` for as long as the process can serve a request. It
/// deliberately checks nothing else: a liveness probe that consults a
/// dependency turns that dependency's outage into a restart loop.
///
/// # Returns
///
/// The static body `"ok"`.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness probe.
///
/// Reports whether the process can serve traffic. From milestone M3 this
/// verifies that object storage is reachable, since a service that cannot read
/// its cache tiers can accept a request but cannot answer it. Until the
/// storage layer exists there is nothing to check and it mirrors liveness.
///
/// # Returns
///
/// The static body `"ready"`.
async fn readyz() -> &'static str {
    "ready"
}
