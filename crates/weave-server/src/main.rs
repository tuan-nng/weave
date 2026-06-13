use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use weave_server::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

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

pub async fn run(config: Config) -> anyhow::Result<()> {
    if config.host != "127.0.0.1" && config.host != "localhost" && !config.allow_remote {
        anyhow::bail!(
            "Binding to non-localhost address '{}' requires --allow-remote flag. \
             This is a safety measure to prevent accidental exposure to the network.",
            config.host
        );
    }

    let db = Arc::new(db::Db::open(&config.db_path)?);

    store::workspaces::WorkspaceStore::ensure_default(&db)?;

    let registry = Arc::new(agent::registry::ProviderRegistry::new());
    let loaded = registry.load_from_db(&db)?;
    info!(loaded, "Providers loaded into registry");

    let mut specialist_registry = specialist::SpecialistRegistry::new();
    let (specialist_loaded, specialist_skipped) =
        specialist_registry.load_from_dir(std::path::Path::new("resources/specialists"));
    let specialists = Arc::new(specialist_registry);
    info!(
        loaded = specialist_loaded,
        skipped = specialist_skipped,
        "Specialists loaded"
    );

    let tools = Arc::new(build_tool_registry(db.clone()));

    let reaped = service::startup::reap_orphans(&db)?;
    if reaped > 0 {
        warn!(count = reaped, "Recovered sessions from previous run");
    }

    let cli_reaped = service::startup::reap_cli_processes(&db)?;
    if cli_reaped.terminated > 0 {
        warn!(
            terminated = cli_reaped.terminated,
            failed = cli_reaped.failed,
            "Reaped orphan CLI processes from previous run"
        );
    }

    let a2a_token = std::env::var("WEAVE_A2A_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if a2a_token.is_some() {
        info!("A2A token authentication enabled");
    } else {
        info!("A2A token authentication disabled (WEAVE_A2A_TOKEN not set)");
    }

    let a2a_default_runtime_kind = match std::env::var("WEAVE_A2A_DEFAULT_RUNTIME_KIND") {
        Ok(raw) if !raw.is_empty() => match raw.parse::<agent::RuntimeKind>() {
            Ok(kind) => {
                info!(
                    runtime_kind = %kind,
                    "A2A default runtime kind set from WEAVE_A2A_DEFAULT_RUNTIME_KIND"
                );
                kind
            }
            Err(e) => {
                warn!(
                    value = %raw,
                    error = %e,
                    "WEAVE_A2A_DEFAULT_RUNTIME_KIND is not a valid runtime kind; \
                     falling back to anthropic-api"
                );
                agent::RuntimeKind::default()
            }
        },
        _ => {
            info!(
                runtime_kind = %agent::RuntimeKind::default(),
                "A2A default runtime kind is anthropic-api (WEAVE_A2A_DEFAULT_RUNTIME_KIND not set)"
            );
            agent::RuntimeKind::default()
        }
    };

    let active_sessions = Arc::new(service::ActiveSessions::new());
    let active_child_processes = Arc::new(service::ActiveChildProcesses::new());
    let sse_manager = Arc::new(sse::SseManager::with_db(db.clone()));
    let shutdown_token = CancellationToken::new();
    let state = AppState {
        db: db.clone(),
        registry,
        active_sessions: active_sessions.clone(),
        active_child_processes: active_child_processes.clone(),
        sse_manager: sse_manager.clone(),
        specialists,
        tools,
        a2a_token,
        a2a_default_runtime_kind,
        shutdown_token: shutdown_token.clone(),
    };
    let start_time = api::health::ServerStartTime(Instant::now());
    let app = api::router(state.clone(), start_time);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Server listening");

    let cleanup_handle = tokio::spawn(run_cleanup(state.clone()));

    // feat-067: spawn the kanban-auto-spawned session lifecycle
    // supervisor. Runs every 30s; scans `kanban_session_watch` for
    // stalled sessions and either re-prompts them or fails them
    // after the recovery budget is exhausted.
    let supervisor_handle = service::kanban_lifecycle::start(state.clone(), shutdown_token.clone());

    let drain_cap = shutdown_drain_cap_from_env();
    if let Some(c) = drain_cap {
        info!(cap_secs = c.as_secs(), "Drain cap active");
    }
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal_with_cap(shutdown_token.clone(), drain_cap));

    server.await?;
    info!("HTTP server returned, awaiting cleanup task");

    match tokio::time::timeout(CLEANUP_BUDGET, cleanup_handle).await {
        Ok(Ok(())) => info!("Cleanup task finished cleanly"),
        Ok(Err(e)) => warn!(error = %e, "Cleanup task panicked"),
        Err(_) => warn!(
            budget_secs = CLEANUP_BUDGET.as_secs(),
            "Cleanup task exceeded its budget; abandoning"
        ),
    }

    // feat-067: the supervisor's loop is gated on `shutdown_token` —
    // it's already exited by now (cleanup awaited the same token).
    // We still `await` its JoinHandle to surface any panic, and bound
    // the wait by the same CLEANUP_BUDGET so a misbehaving supervisor
    // can't block shutdown indefinitely.
    match tokio::time::timeout(CLEANUP_BUDGET, supervisor_handle).await {
        Ok(Ok(())) => info!("Lifecycle supervisor finished cleanly"),
        Ok(Err(e)) => warn!(error = %e, "Lifecycle supervisor panicked"),
        Err(_) => warn!(
            budget_secs = CLEANUP_BUDGET.as_secs(),
            "Lifecycle supervisor exceeded its budget; abandoning"
        ),
    }

    info!("Server shut down gracefully");
    Ok(())
}

fn build_tool_registry(db: Arc<db::Db>) -> tools::ToolRegistry {
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
    tool_registry.register(Arc::new(tools::task::GetTaskTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::ListTasksTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskStatusTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::task::UpdateTaskFieldsTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::kanban::GetBoardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::CreateCardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::SearchCardsTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::MoveCardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::UpdateCardTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::kanban::UpdateTaskTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::artifact::RequestArtifactTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::artifact::ProvideArtifactTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::artifact::ListArtifactsTool {
        db: db.clone(),
    }));
    tool_registry.register(Arc::new(tools::note::CreateNoteTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::ReadNoteTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::ListNotesTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::SetNoteContentTool { db: db.clone() }));
    tool_registry.register(Arc::new(tools::note::AppendToNoteTool { db: db.clone() }));
    tool_registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_shutdown_signal_with_cap_fires_token_on_timeout() {
        let token = CancellationToken::new();
        let child = token.child_token();
        let cap = Duration::from_millis(50);

        timeout(cap * 2, shutdown_signal_with_cap(token.clone(), Some(cap)))
            .await
            .expect("shutdown_signal_with_cap should return within 2x cap");
        assert!(child.is_cancelled(), "token should be fired on cap");
    }
}
