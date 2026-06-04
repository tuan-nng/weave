# Concurrency Model

## Shared State

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,        // Single connection (WAL allows concurrent reads)
    pub session_service: Arc<SessionService>,
    pub kanban_service: Arc<KanbanService>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub specialist_loader: Arc<SpecialistLoader>,
    pub tool_registry: Arc<ToolRegistry>,
    pub trace_collector: Arc<TraceCollector>,
    pub sse_manager: Arc<SseManager>,
    pub config: Arc<Config>,
}
```

All services are wrapped in `Arc` for shared ownership. `AppState` is `Clone` and passed to every route handler via Axum's `State` extractor.

## Database Access

SQLite with WAL mode supports:
- **Concurrent reads** from multiple threads
- **Single writer** at a time (writes queue behind a busy timeout)

Strategy: `Arc<Mutex<Connection>>` with a 5-second busy timeout. All writes acquire the mutex. Reads also go through the mutex for simplicity — SQLite read performance is not a bottleneck for this workload.

**Alternative considered**: Connection pool (r2d2). Rejected because SQLite WAL mode doesn't benefit from multiple connections — writes are serialized anyway, and reads are already fast.

## Async Task Spawning

When a user sends a prompt, agent communication happens in a spawned tokio task:

```rust
pub async fn send_prompt(&self, session_id: &str, message: &str) -> Result<String> {
    // ... validation and setup ...

    let session_service = self.clone();
    let session_id = session_id.to_string();
    let message = message.to_string();

    tokio::spawn(async move {
        if let Err(e) = session_service.run_agent_loop(&session_id, &message).await {
            tracing::error!("Agent loop failed for session {}: {}", session_id, e);
            session_service.handle_error(&session_id, e).await;
        }
    });

    Ok(message_id)
}
```

`run_agent_loop` streams events from the provider, broadcasts them via SSE, and persists them to the database. The HTTP request returns immediately with the message ID.

## Graceful Shutdown

```rust
tokio::select! {
    _ = signal::ctrl_c() => {
        tracing::info!("Shutdown signal received");
    }
    _ = server_handle => {
        tracing::info!("Server exited");
    }
}

// Graceful shutdown:
// 1. Stop accepting new connections
// 2. Wait for in-flight requests to complete (timeout: 30s)
// 3. Cancel all active agent sessions
// 4. Flush database WAL
// 5. Exit
```
