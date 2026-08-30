//! Shared application state.

use std::sync::Arc;

use metrics_exporter_prometheus::PrometheusHandle;

use crate::admission::Admission;
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
    /// Bounds concurrent renders and coalesces duplicate work for one key.
    pub admission: Admission,
    /// Handle used to render the Prometheus scrape.
    ///
    /// `None` when no recorder is installed. A recorder is global and can only
    /// be installed once per process, so tests — which build many states in
    /// one process — leave this empty rather than racing to install it.
    pub metrics: Option<PrometheusHandle>,
}

impl AppState {
    /// Assembles state from configuration, without a metrics recorder.
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
        let admission = config
            .render_concurrency
            .map_or_else(Admission::for_this_machine, Admission::new);

        Ok(Self {
            config: Arc::new(config),
            storage,
            http,
            admission,
            metrics: None,
        })
    }

    /// Attaches a Prometheus render handle.
    #[must_use]
    pub fn with_metrics(mut self, handle: PrometheusHandle) -> Self {
        self.metrics = Some(handle);
        self
    }
}
