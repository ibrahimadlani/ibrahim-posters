//! Poster compositing service.
//!
//! Generates custom movie posters by compositing TMDB artwork with a gradient
//! blur, a darkening ramp, a title logo and a row of text-driven badges, and
//! serves the result as an immutably cacheable HTTP resource.
//!
//! # Module boundaries
//!
//! The crate is layered, and one boundary is load-bearing:
//!
//! - [`api`] is async and axum-aware. It owns extraction, status codes and
//!   response shaping, and nothing else.
//! - [`spec`] is pure and synchronous. It merges a request with its preset,
//!   clamps the result and derives a cache key.
//! - [`tmdb`] owns source addressing. A [`tmdb::PosterPath`] is proof that a
//!   path is safe to append to the configured CDN base, which is the whole
//!   SSRF control.
//! - [`render`] is pure, synchronous, and depends only on a resolved
//!   specification and byte buffers. It performs no I/O and knows nothing
//!   about HTTP.
//!
//! The `render` restriction is enforced by the `disallowed-types` entries in
//! `clippy.toml` rather than by convention, because three properties depend on
//! it: rendering is unit-testable without a runtime, two renderer versions can
//! be compared pixel by pixel, and `rayon` can be used inside a render without
//! any risk of blocking a tokio worker.
//!
//! See `PLAN.md` for the full design and `docs/adr/` for the decisions behind
//! it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod admission;
pub mod api;
pub mod config;
pub mod observability;
pub mod render;
pub mod spec;
pub mod state;
pub mod storage;
pub mod tmdb;

/// Version of the rendering pipeline, mixed into every cache key.
///
/// Rendered posters are served with a one-year `immutable` cache directive and
/// there is no invalidation path, so a change in rendering output must change
/// the keys that output is stored under. Bumping this constant does exactly
/// that: it orphans every derived key at once and forces a re-render.
///
/// **Any change that alters rendered pixels must bump this.** CI enforces it —
/// a visual regression diff without a bump here is a build failure — but the
/// reason is worth stating where the constant lives: without the bump, the CDN
/// keeps serving posters produced by the previous renderer for up to a year,
/// and nothing can correct it.
///
/// See `docs/adr/0005-content-addressed-cache-keys.md`.
pub const RENDER_VERSION: u32 = 2;
