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
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse()
        .context("BIND_ADDR is not a valid socket address")?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    // Report the resolved address rather than the requested one: binding port 0
    // is the usual way to run tests and local instances side by side, and the
    // requested value would be useless in that case.
    let bound = listener
        .local_addr()
        .context("failed to read local address")?;
    tracing::info!(address = %bound, "listening");

    // In-memory until the configuration layer lands in M5; nothing yet
    // reads or writes anything that must outlive the process.
    let storage = poster_service::storage::Storage::in_memory();

    axum::serve(listener, poster_service::api::router(storage))
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
fn init_tracing() {
    let filter = EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);

    if std::env::var("LOG_FORMAT").as_deref() == Ok("pretty") {
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
