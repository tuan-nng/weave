pub mod a2a;
pub mod agent;
pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod service;
pub mod specialist;
pub mod sse;
pub mod store;
pub mod tools;
pub mod trace;

pub use config::Config;
pub use std::sync::Arc;
pub use std::time::{Duration, Instant};
pub use tokio_util::sync::CancellationToken;

pub const CLEANUP_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub registry: Arc<agent::registry::ProviderRegistry>,
    pub active_sessions: Arc<service::ActiveSessions>,
    pub active_child_processes: Arc<service::ActiveChildProcesses>,
    pub sse_manager: Arc<sse::SseManager>,
    pub specialists: Arc<specialist::SpecialistRegistry>,
    pub tools: Arc<tools::ToolRegistry>,
    pub a2a_token: Option<String>,
    pub a2a_default_runtime_kind: agent::RuntimeKind,
    pub shutdown_token: CancellationToken,
}

pub fn shutdown_drain_cap_from_env() -> Option<Duration> {
    let raw = std::env::var("WEAVE_SHUTDOWN_DRAIN_CAP_SECS").ok()?;
    let secs: u64 = raw.parse().ok()?;
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    }
}

pub async fn shutdown_signal_with_cap(token: CancellationToken, cap: Option<Duration>) {
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

    let reason = match cap {
        Some(c) => tokio::select! {
            _ = ctrl_c => "sigint",
            _ = terminate => "sigterm",
            _ = tokio::time::sleep(c) => "drain_cap",
        },
        None => tokio::select! {
            _ = ctrl_c => "sigint",
            _ = terminate => "sigterm",
        },
    };

    tracing::info!(reason = reason, "Shutdown signal received");
    token.cancel();
}

pub async fn run_cleanup(state: AppState) {
    state.shutdown_token.cancelled().await;

    let cancelled = state.active_sessions.cancel_all();
    tracing::info!(cancelled, "Cancelled active sessions");

    let notified = state.sse_manager.broadcast_shutdown("server_shutdown");
    tracing::info!(notified, "Broadcast shutdown to SSE subscribers");

    match state.db.checkpoint() {
        Ok(result) => tracing::info!(
            busy = result.busy,
            log_pages = result.log_pages,
            checkpointed_pages = result.checkpointed_pages,
            "WAL checkpoint complete"
        ),
        Err(e) => tracing::warn!(error = %e, "WAL checkpoint failed during shutdown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::SseWireEvent;
    use crate::store::kanban_test_helpers::make_test_state;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let state = make_test_state();

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

        let state_for_task = state.clone();
        let handle = tokio::spawn(async move { run_cleanup(state_for_task).await });

        tokio::time::sleep(Duration::from_millis(10)).await;
        state.shutdown_token.cancel();

        timeout(CLEANUP_BUDGET, handle)
            .await
            .expect("cleanup exceeded CLEANUP_BUDGET")
            .expect("cleanup task panicked");

        assert!(
            token_a.is_cancelled(),
            "session-a token should be cancelled"
        );
        assert!(
            token_b.is_cancelled(),
            "session-b token should be cancelled"
        );

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

        let result = state.db.checkpoint().expect("checkpoint succeeds");
        assert_eq!(result.log_pages, 0, ":memory: has no WAL");
    }

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
