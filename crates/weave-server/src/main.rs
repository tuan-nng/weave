mod agent;
mod api;
mod config;
mod db;
mod error;
mod service;
mod specialist;
mod sse;
mod store;
mod tools;
mod trace;

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
    pub registry: Arc<agent::registry::ProviderRegistry>,
    pub active_sessions: Arc<service::ActiveSessions>,
    pub sse_manager: Arc<sse::SseManager>,
    pub specialists: Arc<specialist::SpecialistRegistry>,
    pub tools: Arc<tools::ToolRegistry>,
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

    // 3.5 Seed default workspace (idempotent)
    store::workspaces::WorkspaceStore::ensure_default(&db)?;

    // 3.6 Load providers from DB into registry
    let registry = Arc::new(agent::registry::ProviderRegistry::new());
    let loaded = registry.load_from_db(&db)?;
    info!(loaded, "Providers loaded into registry");

    // 3.7 Load specialists from resources/specialists/
    let mut specialist_registry = specialist::SpecialistRegistry::new();
    let (specialist_loaded, specialist_skipped) =
        specialist_registry.load_from_dir(std::path::Path::new("resources/specialists"));
    let specialists = Arc::new(specialist_registry);
    info!(
        loaded = specialist_loaded,
        skipped = specialist_skipped,
        "Specialists loaded"
    );

    // 3.8 Initialize tool registry with filesystem tools
    let mut tool_registry = tools::ToolRegistry::new();
    tool_registry.register(Arc::new(tools::fs::FsReadTool));
    tool_registry.register(Arc::new(tools::fs::FsWriteTool));
    tool_registry.register(Arc::new(tools::fs::FsEditTool));
    tool_registry.register(Arc::new(tools::fs::FsSearchTool));
    tool_registry.register(Arc::new(tools::fs::FsListTool));
    tool_registry.register(Arc::new(tools::shell::ShellExecTool));
    tool_registry.register(Arc::new(tools::git::GitStatusTool));
    tool_registry.register(Arc::new(tools::git::GitDiffTool));
    tool_registry.register(Arc::new(tools::git::GitLogTool));
    tool_registry.register(Arc::new(tools::git::GitCommitTool));
    // Task context tools (first tools with DB access)
    tool_registry.register(Arc::new(tools::task::GetTaskTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::ListTasksTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskStatusTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskFieldsTool {
        db: db.clone(),
    }));
    let tools = Arc::new(tool_registry);

    // 4. Validate remote binding
    if config.host != "127.0.0.1" && config.host != "localhost" && !config.allow_remote {
        anyhow::bail!(
            "Binding to non-localhost address '{}' requires --allow-remote flag. \
             This is a safety measure to prevent accidental exposure to the network.",
            config.host
        );
    }

    // 5. Build the API router
    let active_sessions = Arc::new(service::ActiveSessions::new());
    let sse_manager = Arc::new(sse::SseManager::new());
    let state = AppState {
        db,
        registry,
        active_sessions,
        sse_manager,
        specialists,
        tools,
    };
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
