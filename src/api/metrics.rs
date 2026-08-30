//! The Prometheus scrape endpoint.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::observability;
use crate::state::AppState;

/// Renders the current metric values in Prometheus text format.
///
/// Gauges are sampled here rather than being updated on every state change.
/// A gauge that is written on each acquire and release is both more contended
/// and less accurate — it records the value at some past moment, whereas
/// reading it at scrape time reports what is true when the question is asked.
///
/// # Returns
///
/// `text/plain` in Prometheus exposition format, or `503` if no recorder was
/// installed.
pub async fn scrape(State(state): State<AppState>) -> Response {
    // Counts are bounded by the core count and by the number of distinct keys
    // in flight, both far inside f64's exact integer range. u32 first so the
    // conversion is lossless by construction rather than by argument.
    let as_f64 = |value: usize| f64::from(u32::try_from(value).unwrap_or(u32::MAX));
    metrics::gauge!(observability::SLOTS_AVAILABLE).set(as_f64(state.admission.available()));
    metrics::gauge!(observability::INFLIGHT_KEYS).set(as_f64(state.admission.inflight_keys()));

    let Some(handle) = state.metrics.as_ref() else {
        // Reachable only when the recorder failed to install, which happens
        // if another one is already registered in the process. Tests build
        // state without one.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics recorder is not installed",
        )
            .into_response();
    };

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        handle.render(),
    )
        .into_response()
}
