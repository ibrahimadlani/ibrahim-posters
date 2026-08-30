//! Binary entrypoint.
//!
//! Startup order is deliberate: tracing is initialised before anything that
//! could fail, so that a configuration error is reported through the same
//! subscriber as everything else rather than to a bare stderr write.
//!
//! This is the only module that uses `anyhow`. Startup failures have exactly
//! one consumer — a human reading a log line — so a typed error hierarchy
//! would buy nothing here, whereas in the library every error is matched on.

use std::net::SocketAddr;

use anyhow::Context as _;
use poster_service::config::Config;
use poster_service::state::AppState;
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Loaded before tracing is configured because the log format is itself a
    // configuration value. A failure here is reported by the error return.
    let config = Config::from_env().context("could not load configuration")?;
    init_tracing(&config);

    let addr: SocketAddr = config.bind_addr.parse().with_context(|| {
        format!(
            "BIND_ADDR is not a valid socket address: {}",
            config.bind_addr
        )
    })?;

    let storage = config.build_storage().context("could not open storage")?;
    let backend = config.storage_backend;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    // Report the resolved address rather than the requested one: binding port 0
    // is the usual way to run tests and local instances side by side, and the
    // requested value would be useless in that case.
    let bound = listener
        .local_addr()
        .context("failed to read local address")?;
    tracing::info!(address = %bound, ?backend, "listening");

    // Installed after the listener binds so a bind failure does not leave a
    // global recorder registered in a process that is about to exit.
    let metrics = poster_service::observability::install()
        .context("could not install the metrics recorder")?;

    let state = AppState::new(config, storage)
        .context("could not build the upstream client")?
        .with_metrics(metrics);

    tracing::info!(
        render_concurrency = state.admission.capacity(),
        "admission control ready"
    );

    axum::serve(listener, poster_service::api::router(&state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

/// Initialises the tracing subscriber.
///
/// Emits JSON unless `LOG_FORMAT=pretty`. JSON is the default because the
/// deployed environment is the one that cannot be reconfigured after the fact,
/// so it is the one that should not need an environment variable set correctly
/// to produce parseable output.
fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);

    if config.log_format == "pretty" {
        registry
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    } else {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    }
}

/// Resolves when the process is asked to terminate.
///
/// Handles SIGTERM as well as SIGINT: an orchestrator sends SIGTERM, and a
/// process that only handles SIGINT is killed after the grace period instead
/// of draining. In-flight renders are not cheap to discard, so draining is
/// worth the handler.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received SIGINT, draining"),
        () = terminate => tracing::info!("received SIGTERM, draining"),
    }
}
