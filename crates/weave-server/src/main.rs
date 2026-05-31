mod api;
mod config;
mod db;
mod error;

use clap::Parser;
use config::Config;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Shared application state injected into Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parse CLI args
    let config = Config::parse();

    // 2. Initialize tracing subscriber with RUST_LOG env filter
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        host = %config.host,
        port = config.port,
        db_path = %config.db_path.display(),
        "Starting weave-server"
    );

    // 3. Open database and run migrations
    let db = Arc::new(db::Db::open(&config.db_path)?);

    // 4. Validate remote binding
    if config.host != "127.0.0.1" && config.host != "localhost" && !config.allow_remote {
        anyhow::bail!(
            "Binding to non-localhost address '{}' requires --allow-remote flag. \
             This is a safety measure to prevent accidental exposure to the network.",
            config.host
        );
    }

    // 5. Build the API router
    let state = AppState { db };
    let start_time = api::health::ServerStartTime(Instant::now());
    let app = api::router(state, start_time);

    // 6. Bind and listen
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Server listening");

    // 7. Graceful shutdown on SIGTERM/SIGINT
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

    server.await?;

    info!("Server shut down gracefully");
    Ok(())
}

/// Wait for SIGTERM or SIGINT.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received SIGINT, shutting down...");
        }
        _ = terminate => {
            info!("Received SIGTERM, shutting down...");
        }
    }
}
