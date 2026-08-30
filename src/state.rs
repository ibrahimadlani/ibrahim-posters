//! Shared application state.

use std::sync::Arc;

use crate::config::Config;
use crate::storage::Storage;

/// State cloned into every request handler.
///
/// Every field is `Arc`-wrapped or internally shared, so `Clone` is a refcount
/// bump rather than a copy.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Configuration, immutable after startup.
    pub config: Arc<Config>,
    /// Object storage for both cache tiers and specification persistence.
    pub storage: Storage,
    /// Upstream client: explicit timeout, redirects refused.
    pub http: reqwest::Client,
}

impl AppState {
    /// Assembles state from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` — validated configuration.
    /// * `storage` — the storage layer `config` describes.
    ///
    /// # Errors
    ///
    /// Returns the underlying `reqwest` error if the TLS backend cannot be
    /// initialised.
    pub fn new(config: Config, storage: Storage) -> Result<Self, reqwest::Error> {
        let http = crate::tmdb::fetch::client(config.fetch_limits())?;
        Ok(Self {
            config: Arc::new(config),
            storage,
            http,
        })
    }
}
