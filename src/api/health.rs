//! Liveness and readiness probes.
//!
//! The two are deliberately distinct. Liveness answers "is this process
//! wedged, should the orchestrator restart it"; readiness answers "can this
//! process serve traffic right now". Conflating them causes an orchestrator to
//! restart a healthy process because a dependency it does not own is briefly
//! unavailable — which removes capacity at precisely the moment the system is
//! already degraded.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{routing::get, Router};

use crate::storage::Storage;

/// Mounts the health routes.
///
/// # Arguments
///
/// * `storage` — consulted by the readiness probe.
///
/// # Returns
///
/// A [`Router`] exposing `GET /healthz` and `GET /readyz`.
pub fn routes(storage: Storage) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(storage)
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
/// Reports whether the process can serve traffic, which here means whether
/// object storage is reachable: the service can accept a request without it
/// but cannot answer one, since both cache tiers and every stored
/// specification live there.
///
/// # Returns
///
/// `200 OK` with body `"ready"`, or `503 Service Unavailable` with a short
/// reason when storage cannot be reached.
async fn readyz(State(storage): State<Storage>) -> (StatusCode, &'static str) {
    match storage.check_reachable().await {
        Ok(()) => (StatusCode::OK, "ready"),
        Err(error) => {
            // Logged rather than returned: the probe's consumer is an
            // orchestrator that acts on the status code, and the detail
            // belongs where an operator will look for it.
            tracing::warn!(%error, "readiness check failed: storage unreachable");
            (StatusCode::SERVICE_UNAVAILABLE, "storage unreachable")
        }
    }
}
