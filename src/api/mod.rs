//! HTTP surface: routing, middleware and response shaping.
//!
//! This module is the only place that knows about axum. Everything below it
//! deals in domain types and byte buffers, which is what keeps the renderer
//! testable without a runtime.

pub mod health;

use axum::Router;

/// Builds the application router with its middleware stack.
///
/// # Returns
///
/// A [`Router`] with every route mounted. The caller is responsible for
/// binding a listener and serving it.
///
/// # Examples
///
/// ```
/// let app = poster_service::api::router();
/// // `app` is ready to hand to `axum::serve`.
/// let _ = app;
/// ```
pub fn router() -> Router {
    Router::new().merge(health::routes())
}
