//! HTTP surface: routing, middleware and response shaping.
//!
//! This module is the only place that knows about axum. Everything below it
//! deals in domain types and byte buffers, which is what keeps the renderer
//! testable without a runtime.

pub mod health;

use axum::Router;

use crate::storage::Storage;

/// Builds the application router with its middleware stack.
///
/// # Arguments
///
/// * `storage` — the object store backing both cache tiers and specification
///   persistence.
///
/// # Returns
///
/// A [`Router`] with every route mounted. The caller is responsible for
/// binding a listener and serving it.
///
/// # Examples
///
/// ```
/// use poster_service::{api, storage::Storage};
///
/// let app = api::router(Storage::in_memory());
/// // `app` is ready to hand to `axum::serve`.
/// let _ = app;
/// ```
pub fn router(storage: Storage) -> Router {
    Router::new().merge(health::routes(storage))
}
