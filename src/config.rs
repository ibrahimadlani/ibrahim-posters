//! Runtime configuration.
//!
//! Loaded from `POSTER_`-prefixed environment variables via figment.
//!
//! # Why the prefix
//!
//! The first version read unprefixed names and rejected unknown fields. Those
//! two together do not work: figment's raw environment provider merges *every*
//! variable in the process, so `deny_unknown_fields` failed on the first
//! unrelated one it found — the service would not have started in any real
//! environment. A prefix scopes the strictness to variables intended for this
//! service, which keeps the useful half of the behaviour: a misspelled
//! `POSTER_` variable is an error at startup rather than a setting silently
//! ignored.
//!
//! # Why AWS credentials are not here
//!
//! They are read from the standard unprefixed `AWS_*` variables by
//! `object_store` itself, which also understands IAM roles and instance
//! metadata. Declaring them here would mean a credential living in two places
//! and would break the conventional names every AWS tool already uses.
//!
//! The one secret that does live here is wrapped in [`secrecy::SecretString`],
//! so that `#[derive(Debug)]` on this type — and on
//! [`crate::state::AppState`], which holds it and is attached to tracing spans
//! — cannot put it in a log line.

use std::sync::Arc;
use std::time::Duration;

use figment::providers::Env;
use figment::Figment;
use secrecy::SecretString;
use serde::Deserialize;

use crate::render::encode;
use crate::storage::Storage;
use crate::tmdb::fetch::FetchLimits;

/// Which object store backs the cache tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// Process memory. Nothing survives a restart; intended for tests and for
    /// a development run that should leave nothing behind.
    Memory,
    /// A directory on the local filesystem.
    Local,
    /// S3 or any S3-compatible endpoint.
    S3,
}

/// Why configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A value was missing or malformed.
    #[error("configuration is invalid: {0}")]
    Invalid(String),
    /// A required value for the selected storage backend was absent.
    #[error("{backend:?} storage requires {name} to be set")]
    MissingForBackend {
        /// The selected backend.
        backend: StorageBackend,
        /// The variable that was missing.
        name: &'static str,
    },
    /// The object store could not be constructed.
    #[error("could not open storage: {0}")]
    Storage(String),
}

/// Service configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address to listen on.
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,

    /// `json` in production, `pretty` for a terminal.
    #[serde(default = "default_log_format")]
    pub log_format: String,

    /// Tracing filter directive.
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Which object store to use.
    #[serde(default = "default_storage_backend")]
    pub storage_backend: StorageBackend,

    /// Bucket name, required when `storage_backend` is `s3`.
    pub storage_bucket: Option<String>,

    /// Root directory, required when `storage_backend` is `local`.
    pub storage_local_path: Option<String>,

    /// Endpoint override for S3-compatible hosts such as `MinIO` or R2.
    pub s3_endpoint: Option<String>,

    /// Region override.
    ///
    /// Optional: `object_store` reads `AWS_REGION` from the environment
    /// itself. This exists so a deployment can override it without touching
    /// the AWS-conventional variables.
    pub s3_region: Option<String>,

    /// TMDB CDN base. The only host the service ever contacts.
    #[serde(default = "default_tmdb_base")]
    pub tmdb_image_base: String,

    /// TMDB API key.
    ///
    /// **Not required.** Clients supply `poster_path` directly and
    /// `image.tmdb.org` serves artwork unauthenticated. This exists only for a
    /// future endpoint that resolves a TMDB *id* to a path, which v1 does not
    /// do. It is declared so the omission is visibly deliberate: a service
    /// that needs no credential to fetch its primary input is a meaningfully
    /// smaller thing to operate.
    pub tmdb_api_key: Option<SecretString>,

    /// Public origin used to build the URL returned by `POST /v1/posters`.
    ///
    /// Set to the CDN origin in production. Without it the response carries a
    /// path rather than an absolute URL, which is correct but less convenient.
    pub public_base_url: Option<String>,

    /// Streaming byte cap on upstream responses.
    #[serde(default = "default_max_upstream_bytes")]
    pub max_upstream_bytes: usize,

    /// Largest accepted source dimension, per side.
    #[serde(default = "default_max_source_dimension")]
    pub max_source_dimension: u32,

    /// Total upstream request timeout, in milliseconds.
    #[serde(default = "default_upstream_timeout_ms")]
    pub upstream_timeout_ms: u64,

    /// Concurrent renders permitted.
    ///
    /// Defaults to the machine's core count. Renders are CPU-bound, so
    /// admitting more work than there are cores raises latency without raising
    /// throughput; the setting exists for containers whose CPU quota does not
    /// match the core count the process can see.
    pub render_concurrency: Option<usize>,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_owned()
}
fn default_log_format() -> String {
    "json".to_owned()
}
fn default_log_level() -> String {
    "info".to_owned()
}
fn default_storage_backend() -> StorageBackend {
    StorageBackend::Memory
}
fn default_tmdb_base() -> String {
    "https://image.tmdb.org/t/p".to_owned()
}
fn default_max_upstream_bytes() -> usize {
    20 * 1024 * 1024
}
fn default_max_source_dimension() -> u32 {
    8000
}
fn default_upstream_timeout_ms() -> u64 {
    3000
}

impl Config {
    /// Prefix every service variable carries.
    pub const ENV_PREFIX: &'static str = "POSTER_";

    /// Loads configuration from `POSTER_`-prefixed environment variables.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] if a value is malformed, or if a
    /// `POSTER_`-prefixed variable does not name a known setting.
    pub fn from_env() -> Result<Self, ConfigError> {
        Figment::new()
            .merge(Env::prefixed(Self::ENV_PREFIX))
            .extract()
            .map_err(|error| ConfigError::Invalid(error.to_string()))
    }

    /// Returns the configuration a process with no `POSTER_` variables set
    /// would receive.
    ///
    /// In-memory storage, no upstream credential, the public TMDB CDN. Used by
    /// tests, and useful for confirming what the defaults actually are.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            log_format: default_log_format(),
            log_level: default_log_level(),
            storage_backend: default_storage_backend(),
            storage_bucket: None,
            storage_local_path: None,
            s3_endpoint: None,
            s3_region: None,
            tmdb_image_base: default_tmdb_base(),
            tmdb_api_key: None,
            public_base_url: None,
            max_upstream_bytes: default_max_upstream_bytes(),
            max_source_dimension: default_max_source_dimension(),
            upstream_timeout_ms: default_upstream_timeout_ms(),
            render_concurrency: None,
        }
    }

    /// Returns the fetch limits these settings imply.
    #[must_use]
    pub fn fetch_limits(&self) -> FetchLimits {
        FetchLimits {
            max_bytes: self.max_upstream_bytes,
            max_dimension: self.max_source_dimension,
            timeout: Duration::from_millis(self.upstream_timeout_ms),
        }
    }

    /// Builds the storage layer these settings describe.
    ///
    /// # Errors
    ///
    /// [`ConfigError::MissingForBackend`] if the selected backend needs a
    /// value that was not supplied, and [`ConfigError::Storage`] if the store
    /// cannot be opened.
    pub fn build_storage(&self) -> Result<Storage, ConfigError> {
        match self.storage_backend {
            StorageBackend::Memory => Ok(Storage::in_memory()),
            StorageBackend::Local => {
                let path =
                    self.storage_local_path
                        .as_deref()
                        .ok_or(ConfigError::MissingForBackend {
                            backend: StorageBackend::Local,
                            name: "STORAGE_LOCAL_PATH",
                        })?;
                // Created rather than required to exist: a fresh local run
                // should not need a mkdir first.
                std::fs::create_dir_all(path)
                    .map_err(|error| ConfigError::Storage(error.to_string()))?;
                let store = object_store::local::LocalFileSystem::new_with_prefix(path)
                    .map_err(|error| ConfigError::Storage(error.to_string()))?;
                Ok(Storage::new(Arc::new(store)))
            }
            StorageBackend::S3 => {
                let bucket =
                    self.storage_bucket
                        .as_deref()
                        .ok_or(ConfigError::MissingForBackend {
                            backend: StorageBackend::S3,
                            name: "STORAGE_BUCKET",
                        })?;

                let mut builder =
                    object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
                if let Some(region) = &self.s3_region {
                    builder = builder.with_region(region);
                }
                if let Some(endpoint) = &self.s3_endpoint {
                    // A custom endpoint implies path-style addressing; virtual
                    // host style requires DNS the operator does not control.
                    builder = builder
                        .with_endpoint(endpoint)
                        .with_virtual_hosted_style_request(false);
                }

                let store = builder
                    .build()
                    .map_err(|error| ConfigError::Storage(error.to_string()))?;
                Ok(Storage::new(Arc::new(store)))
            }
        }
    }

    /// Builds the absolute or relative URL for a rendered poster.
    ///
    /// # Arguments
    ///
    /// * `key` — the poster's content-addressed key, in hex.
    #[must_use]
    pub fn poster_url(&self, key: &str) -> String {
        let path = format!("/v1/posters/{key}.webp");
        match &self.public_base_url {
            Some(base) => format!("{}{path}", base.trim_end_matches('/')),
            None => path,
        }
    }

    /// Quality used for the L1 resized-source tier.
    #[must_use]
    pub const fn intermediate_quality(&self) -> f32 {
        encode::INTERMEDIATE_QUALITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        Config::defaults()
    }

    #[test]
    fn defaults_are_usable_without_any_environment() {
        let config = base();
        assert!(config.build_storage().is_ok());
        assert_eq!(config.fetch_limits().max_dimension, 8000);
    }

    #[test]
    fn secrets_do_not_appear_in_debug_output() {
        // Config is held by AppState, which is attached to tracing spans. A
        // derived Debug over a plain String would put the key in logs.
        let mut config = base();
        config.tmdb_api_key = Some(SecretString::from("super-secret-value"));

        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("super-secret-value"),
            "the TMDB key leaked into Debug output"
        );
    }

    #[test]
    fn loading_ignores_variables_that_are_not_ours() {
        // The failure this prefix exists to prevent: reading unprefixed names
        // meant figment merged every variable in the process, and
        // deny_unknown_fields rejected the first unrelated one it found. The
        // service would not have started in any real environment.
        assert!(
            std::env::vars().count() > 5,
            "the test environment has no variables to be confused by"
        );
        assert!(
            Config::from_env().is_ok(),
            "loading must ignore variables outside the {} namespace",
            Config::ENV_PREFIX
        );
    }

    #[test]
    fn a_misspelled_setting_is_an_error_rather_than_a_silent_default() {
        // The half of the strictness worth keeping. Tested through figment
        // directly rather than through the process environment, which tests
        // must not mutate concurrently.
        let result: Result<Config, _> = Figment::new()
            .merge(("storage_backend_typo", "s3"))
            .extract::<Config>();

        assert!(result.is_err(), "a misspelled key was silently ignored");
    }

    #[test]
    fn s3_without_a_bucket_is_rejected_by_name() {
        let mut config = base();
        config.storage_backend = StorageBackend::S3;

        let error = config.build_storage().expect_err("must require a bucket");
        assert!(
            matches!(
                error,
                ConfigError::MissingForBackend {
                    name: "STORAGE_BUCKET",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn local_without_a_path_is_rejected_by_name() {
        let mut config = base();
        config.storage_backend = StorageBackend::Local;

        let error = config.build_storage().expect_err("must require a path");
        assert!(
            matches!(
                error,
                ConfigError::MissingForBackend {
                    name: "STORAGE_LOCAL_PATH",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn a_local_backend_creates_its_directory() {
        let mut config = base();
        let path = std::env::temp_dir().join(format!("poster-config-{}", std::process::id()));
        config.storage_backend = StorageBackend::Local;
        config.storage_local_path = Some(path.to_string_lossy().into_owned());

        assert!(config.build_storage().is_ok());
        assert!(path.exists(), "the directory was not created");
        std::fs::remove_dir_all(&path).expect("cleans up");
    }

    #[test]
    fn poster_urls_are_absolute_when_an_origin_is_configured() {
        let mut config = base();
        assert_eq!(config.poster_url("abc"), "/v1/posters/abc.webp");

        config.public_base_url = Some("https://cdn.example.test/".to_owned());
        assert_eq!(
            config.poster_url("abc"),
            "https://cdn.example.test/v1/posters/abc.webp",
            "a trailing slash on the origin must not double up"
        );
    }

    #[test]
    fn fetch_limits_follow_configuration() {
        let mut config = base();
        config.max_upstream_bytes = 1234;
        config.upstream_timeout_ms = 500;

        let limits = config.fetch_limits();
        assert_eq!(limits.max_bytes, 1234);
        assert_eq!(limits.timeout, Duration::from_millis(500));
    }
}
