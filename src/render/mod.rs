//! Poster compositing.
//!
//! # Boundary
//!
//! This module and everything under it is **pure and synchronous**. It takes a
//! [`crate::spec::ResolvedSpec`] and byte buffers, and returns pixels. It has
//! no `async fn`, and no dependency on `reqwest`, `object_store`, `axum` or
//! `tokio`.
//!
//! Three properties follow, and they are the reason the rule exists:
//!
//! 1. Every rendering behaviour is unit-testable without a runtime, a network
//!    stub or a temporary directory.
//! 2. Two versions of the renderer can be run over identical inputs and
//!    compared pixel by pixel, which is what makes the visual regression gate
//!    meaningful.
//! 3. `rayon` can be used freely, because the whole call already sits inside
//!    `spawn_blocking` and never touches a tokio worker.
//!
//! The rule is enforced by `tests/module_boundary.rs`, which fails the build
//! if any of those names appears in this directory.

pub mod badges;
pub mod blur;
pub mod encode;
pub mod fonts;
pub mod gradient;
pub mod logo;
pub mod pipeline;
pub mod source;

pub use pipeline::{render, render_encoded, Assets};

/// Why a render failed.
///
/// Every variant is a bug in the input rather than a transient condition:
/// nothing here is worth retrying, because the same specification and the
/// same bytes produce the same failure every time.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// Source artwork could not be decoded.
    #[error("could not decode source artwork: {0}")]
    Decode(String),
    /// Resampling failed.
    #[error("could not resize source artwork: {0}")]
    Resize(String),
    /// A pixel buffer did not match its stated dimensions.
    #[error("buffer is {found} bytes, expected {expected}")]
    MalformedBuffer {
        /// Length implied by the dimensions.
        expected: usize,
        /// Length actually supplied.
        found: usize,
    },
    /// Badge text could not be laid out or rasterised.
    #[error("could not render badges: {0}")]
    Badges(String),
    /// WebP encoding failed.
    #[error("could not encode output: {0}")]
    Encode(String),
}
