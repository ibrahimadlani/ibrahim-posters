//! Request resolution: merge, clamp, canonicalise, hash.
//!
//! This module is pure and synchronous. It performs no I/O and knows nothing
//! about HTTP, which is what lets the whole resolution path be tested without
//! a runtime or a network stub.
//!
//! # The pipeline
//!
//! ```text
//! PosterRequest  --[preset merge]-->  --[clamp]-->  ResolvedSpec  --[hash]-->  CacheKey
//! ```
//!
//! # Invariant
//!
//! Merge happens before clamping, never the other way round. Clamping an
//! override first would let two requests that clamp to the same value survive
//! as distinct specifications, producing two cache keys for one image. At the
//! target hit rate that is the difference between the service being cheap and
//! being several times more expensive than it needs to be.

pub mod clamp;
pub mod key;
pub mod preset;
pub mod request;
pub mod resolved;

pub use key::CacheKey;
pub use preset::{resolve, Preset};
pub use request::{Badge, BadgeStyle, OutputWidth, Overrides, PosterRequest, SpecError};
pub use resolved::ResolvedSpec;
