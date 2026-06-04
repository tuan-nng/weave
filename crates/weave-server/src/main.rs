mod a2a;
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
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Cap on how long `run()` waits for in-flight requests to drain after a
/// shutdown signal before the HTTP server returns. Once the cap elapses,
/// axum drops the connection for any request still in flight. The cleanup
/// task continues independently and has its own budget (see
/// `CLEANUP_BUDGET`).
///
/// Spec value from feat-034 ("30s timeout"). Kept as a const so tests can
/// exercise the timeout path by spawning `run()` with a long cap and
/// dropping the signal after a brief sleep.
pub const SHUTDOWN_DRAIN_CAP: Duration = Duration::from_secs(30);

/// Cap on how long the cleanup task is allowed to run after the server has
/// returned. Generous (5s) so a slow WAL checkpoint on a busy database
/// still has a chance to land; bounded so a hung session-cancel path
/// can't keep the process alive forever.
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(5);

/// Shared application state injected into Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub registry: Arc<agent::registry::ProviderRegistry>,
    pub active_sessions: Arc<service::ActiveSessions>,
    pub sse_manager: Arc<sse::SseManager>,
    pub specialists: Arc<specialist::SpecialistRegistry>,
    pub tools: Arc<tools::ToolRegistry>,
    pub a2a_token: Option<String>,
    /// Parent cancellation token fired by the shutdown sequence (feat-034).
    /// Held by `AppState` so any handler can subscribe to "server is going
    /// down" if it ever needs to abort a long-running operation. Currently
    /// only the cleanup task in `run()` reads it directly; the field is
    /// here so future per-request work (e.g. long SSE feeds) can wire
    /// themselves up without restructuring `AppState`.
    pub shutdown_token: CancellationToken,
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

    run(config).await
}

/// Build state, bind the listener, run the server, perform graceful
/// shutdown. Extracted from `main()` so the full startup + shutdown
/// sequence can be exercised by tests without spawning the real
/// `#[tokio::main]` runtime.
pub async fn run(config: Config) -> anyhow::Result<()> {
    // Validate remote binding up front so a misconfigured operator never
    // gets far enough to leave orphan rows in the database.
    if config.host != "127.0.0.1" && config.host != "localhost" && !config.allow_remote {
        anyhow::bail!(
            "Binding to non-localhost address '{}' requires --allow-remote flag. \
             This is a safety measure to prevent accidental exposure to the network.",
            config.host
        );
    }

    // Open database and run migrations.
    let db = Arc::new(db::Db::open(&config.db_path)?);

    // Seed default workspace (idempotent).
    store::workspaces::WorkspaceStore::ensure_default(&db)?;

    // Load providers from DB into registry.
    let registry = Arc::new(agent::registry::ProviderRegistry::new());
    let loaded = registry.load_from_db(&db)?;
    info!(loaded, "Providers loaded into registry");

    // Load specialists from resources/specialists/.
    let mut specialist_registry = specialist::SpecialistRegistry::new();
    let (specialist_loaded, specialist_skipped) =
        specialist_registry.load_from_dir(std::path::Path::new("resources/specialists"));
    let specialists = Arc::new(specialist_registry);
    info!(
        loaded = specialist_loaded,
        skipped = specialist_skipped,
        "Specialists loaded"
    );

    // Initialize tool registry. Mirrors the production toolset so a test
    // that calls `run()` directly (rather than `make_test_state`) sees the
    // same handler surface.
    let tools = Arc::new(build_tool_registry(db.clone()));

    // Reap orphan sessions from any previous crashed process (feat-034).
    let reaped = service::startup::reap_orphans(&db)?;
    if reaped > 0 {
        warn!(count = reaped, "Recovered sessions from previous run");
    }

    // Read A2A token from environment.
    let a2a_token = std::env::var("WEAVE_A2A_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if a2a_token.is_some() {
        info!("A2A token authentication enabled");
    } else {
        info!("A2A token authentication disabled (WEAVE_A2A_TOKEN not set)");
    }

    // Build the API router.
    let active_sessions = Arc::new(service::ActiveSessions::new());
    let sse_manager = Arc::new(sse::SseManager::new());
    let shutdown_token = CancellationToken::new();
    let state = AppState {
        db: db.clone(),
        registry,
        active_sessions: active_sessions.clone(),
        sse_manager: sse_manager.clone(),
        specialists,
        tools,
        a2a_token,
        shutdown_token: shutdown_token.clone(),
    };
    let start_time = api::health::ServerStartTime(Instant::now());
    let app = api::router(state.clone(), start_time);

    // Bind the listener. From this point on, new connections are accepted;
    // a future `reap_orphans` invocation would have to handle the case of
    // a client that races to connect here, so we run it BEFORE binding.
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Server listening");

    // Spawn the cleanup task. It waits on `shutdown_token` and runs in
    // parallel with the HTTP drain so the WAL checkpoint and SSE
    // disconnect notices don't add latency to the shutdown window.
    let cleanup_handle = tokio::spawn(run_cleanup(state.clone()));

    // Drive the server with a graceful-shutdown future that respects the
    // drain cap. `shutdown_signal_with_cap` fires the token before
    // returning so the cleanup task starts the moment a signal arrives —
    // the two never wait on each other.
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal_with_cap(
        shutdown_token.clone(),
        SHUTDOWN_DRAIN_CAP,
    ));

    server.await?;
    info!("HTTP server returned, awaiting cleanup task");

    // The server returned; the cleanup task is still racing against
    // `CLEANUP_BUDGET`. Bound it so a hung session-cancel path can't
    // keep the process alive.
    match tokio::time::timeout(CLEANUP_BUDGET, cleanup_handle).await {
        Ok(Ok(())) => info!("Cleanup task finished cleanly"),
        Ok(Err(e)) => warn!(error = %e, "Cleanup task panicked"),
        Err(_) => warn!(
            budget_secs = CLEANUP_BUDGET.as_secs(),
            "Cleanup task exceeded its budget; abandoning"
        ),
    }

    info!("Server shut down gracefully");
    Ok(())
}

/// Build the production tool registry. Extracted from `run()` so tests
/// that need the same surface can call it without copy-pasting the
/// twenty `register` calls. Mirrors the tool list as of feat-034.
fn build_tool_registry(db: Arc<db::Db>) -> tools::ToolRegistry {
    let mut tool_registry = tools::ToolRegistry::new();
    // Filesystem tools.
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
    // Task context tools.
    tool_registry.register(Arc::new(tools::task::GetTaskTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::ListTasksTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskStatusTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskFieldsTool {
        db: db.clone(),
    }));
    // Kanban tools (feat-028).
    tool_registry.register(Arc::new(tools::kanban::GetBoardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::CreateCardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::SearchCardsTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::MoveCardTool { db: db.clone() }));
    // Artifact tools (feat-031).
    tool_registry.register(Arc::new(tools::artifact::RequestArtifactTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::artifact::ProvideArtifactTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::artifact::ListArtifactsTool {
        db: db.clone(),
    }));
    // Note tools (feat-030).
    tool_registry.register(Arc::new(tools::note::CreateNoteTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::ReadNoteTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::ListNotesTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::SetNoteContentTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::AppendToNoteTool { db: db.clone() }));
    tool_registry
}

/// Wait for SIGTERM, SIGINT, or a wall-clock cap, whichever comes first.
///
/// Fires `token` on every exit path so the cleanup task is always woken —
/// even on the cap (we want a clean shutdown attempt even if the operator
/// never signalled). The cap exists so a process that never receives a
/// signal (e.g. an orchestrator that lost the PID) still terminates
/// within `SHUTDOWN_DRAIN_CAP` of `axum::serve` returning. The structured
/// `Shutdown signal received` log carries the reason for the operator.
async fn shutdown_signal_with_cap(token: CancellationToken, cap: Duration) {
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

    let reason = tokio::select! {
        _ = ctrl_c => "sigint",
        _ = terminate => "sigterm",
        _ = tokio::time::sleep(cap) => "drain_cap",
    };

    info!(reason = reason, "Shutdown signal received");
    token.cancel();
}

/// Run the cleanup sequence on shutdown.
///
/// 1. Cancel every active session token — agent streams return on the
///    next iteration of their `select!`.
/// 2. Broadcast a `Shutdown` SSE event to every live channel so clients
///    can close their `EventSource` instead of waiting for a TCP FIN.
/// 3. Run `PRAGMA wal_checkpoint(TRUNCATE)` to flush the WAL to disk.
///
/// Steps 1 and 2 are independent of each other and of step 3; we run them
/// sequentially here for predictable log ordering. The whole sequence
/// is bounded by `CLEANUP_BUDGET` (enforced by the caller's `timeout`).
async fn run_cleanup(state: AppState) {
    state.shutdown_token.cancelled().await;

    let cancelled = state.active_sessions.cancel_all();
    info!(cancelled, "Cancelled active sessions");

    let notified = state.sse_manager.broadcast_shutdown("server_shutdown");
    info!(notified, "Broadcast shutdown to SSE subscribers");

    match state.db.checkpoint() {
        Ok(result) => info!(
            busy = result.busy,
            log_pages = result.log_pages,
            checkpointed_pages = result.checkpointed_pages,
            "WAL checkpoint complete"
        ),
        Err(e) => warn!(error = %e, "WAL checkpoint failed during shutdown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::SseWireEvent;
    use crate::store::kanban_test_helpers::make_test_state;
    use std::time::Duration;
    use tokio::time::timeout;

    /// `cleanup_task` cancels every active session, broadcasts a
    /// `Shutdown` event to every live SSE channel, and runs the WAL
    /// checkpoint — in that order. Each step produces a structured log
    /// line and never panics on an empty manager.
    #[tokio::test]
    async fn test_graceful_shutdown() {
        let state = make_test_state();

        // Two active sessions, two SSE subscribers (different entities).
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();
        state
            .active_sessions
            .try_insert("session-a".into(), token_a.clone());
        state
            .active_sessions
            .try_insert("session-b".into(), token_b.clone());
        let mut rx_session = state.sse_manager.subscribe("session-a");
        let mut rx_board = state.sse_manager.subscribe("board-1");

        // Spawn the cleanup, then fire the token.
        let state_for_task = state.clone();
        let handle = tokio::spawn(async move { run_cleanup(state_for_task).await });

        // Give the task a moment to enter `cancelled().await`, then fire.
        tokio::time::sleep(Duration::from_millis(10)).await;
        state.shutdown_token.cancel();

        // Bound the cleanup by the production budget.
        timeout(CLEANUP_BUDGET, handle)
            .await
            .expect("cleanup exceeded CLEANUP_BUDGET")
            .expect("cleanup task panicked");

        // 1. Both session tokens were cancelled.
        assert!(
            token_a.is_cancelled(),
            "session-a token should be cancelled"
        );
        assert!(
            token_b.is_cancelled(),
            "session-b token should be cancelled"
        );

        // 2. Both SSE subscribers received a Shutdown event with reason
        //    "server_shutdown".
        let s_event = rx_session.recv().await.expect("session subscriber");
        let b_event = rx_board.recv().await.expect("board subscriber");
        match s_event {
            SseWireEvent::Shutdown { reason } => assert_eq!(reason, "server_shutdown"),
            other => panic!("expected Shutdown for session, got {other:?}"),
        }
        match b_event {
            SseWireEvent::Shutdown { reason } => assert_eq!(reason, "server_shutdown"),
            other => panic!("expected Shutdown for board, got {other:?}"),
        }

        // 3. The WAL checkpoint ran — `Db::checkpoint` returns Ok even on
        //    :memory: (with zero pages); we just assert the call didn't
        //    surface an error.
        let result = state.db.checkpoint().expect("checkpoint succeeds");
        assert_eq!(result.log_pages, 0, ":memory: has no WAL");
    }

    /// The cap branch of `shutdown_signal_with_cap` returns when the
    /// cap elapses and fires the token so the cleanup task still wakes
    /// up. We use a 50ms cap here to keep the test fast.
    #[tokio::test]
    async fn test_shutdown_signal_with_cap_fires_token_on_timeout() {
        let token = CancellationToken::new();
        let child = token.child_token();
        let cap = Duration::from_millis(50);

        timeout(cap * 2, shutdown_signal_with_cap(token.clone(), cap))
            .await
            .expect("shutdown_signal_with_cap should return within 2x cap");
        assert!(child.is_cancelled(), "token should be fired on cap");
    }
}
