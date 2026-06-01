use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::registry::ProviderRegistry;
use crate::agent::{self, MessageRequest};
use crate::db::Db;
use crate::error::AppError;
use crate::specialist::{Specialist, SpecialistRegistry};
use crate::sse::{self, SseWireEvent};
use crate::store::providers::ProviderStore;
use crate::store::sessions::{MessageStore, SessionStore};
use crate::tools::ToolRegistry;

use super::ActiveSessions;

/// Default max tokens for agent responses.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Hardcoded fallback model when neither session nor provider specifies one.
const FALLBACK_MODEL: &str = "claude-sonnet-4-20250514";

/// Maximum number of messages loaded into conversation history.
const MAX_HISTORY_MESSAGES: usize = 1000;

/// Stateless service that orchestrates session prompt lifecycle.
///
/// Validates session state, persists messages, spawns streaming tasks,
/// and supports cancellation. Mirrors the store pattern (unit struct,
/// static methods) but operates at a higher abstraction level.
pub struct SessionService;

impl SessionService {
    /// Send a prompt to a session.
    ///
    /// Returns the user message ID immediately. Spawns an async task that
    /// streams the agent response, accumulates text, and saves the assistant
    /// message when complete.
    pub async fn send_prompt(
        db: &Arc<Db>,
        registry: &Arc<ProviderRegistry>,
        specialists: &Arc<SpecialistRegistry>,
        active: &Arc<ActiveSessions>,
        sse_manager: &Arc<sse::SseManager>,
        tools: &Arc<ToolRegistry>,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, AppError> {
        // Validate prompt is non-empty
        if prompt.trim().is_empty() {
            return Err(AppError::Validation("prompt cannot be empty".into()));
        }

        // Validate session exists and is in a non-terminal state
        let session = SessionStore::get_by_id(db, session_id)?;
        if crate::store::sessions::TERMINAL.contains(&session.status.as_str()) {
            return Err(AppError::Validation(format!(
                "cannot send prompt to session in '{}' status",
                session.status
            )));
        }

        // Validate tool profile early (fail-fast before spawning task)
        if let Some(ref specialist_id) = session.specialist_id {
            if let Some(specialist) = specialists.get_by_name(specialist_id) {
                if let Some(ref profile) = specialist.tool_profile {
                    tools.validate_profile_name(profile)?;
                }
            }
        }

        // Atomically check-and-insert to prevent TOCTOU race
        let cancel_token = CancellationToken::new();
        if !active.try_insert(session_id.to_string(), cancel_token.clone()) {
            return Err(AppError::Conflict(
                "session already has an active prompt".into(),
            ));
        }

        // Save user message (raw text, no JSON encoding)
        let user_msg = MessageStore::create(db, session_id, "user", prompt, None)?;

        // Spawn the streaming task
        let task_db = Arc::clone(db);
        let task_registry = Arc::clone(registry);
        let task_specialists = Arc::clone(specialists);
        let task_active = Arc::clone(active);
        let task_sse = Arc::clone(sse_manager);
        let task_tools = Arc::clone(tools);
        let task_session = session;

        tokio::spawn(async move {
            run_prompt_task(
                task_db,
                task_registry,
                task_specialists,
                task_active,
                task_sse,
                task_tools,
                task_session,
                cancel_token,
            )
            .await;
        });

        Ok(user_msg.id)
    }

    /// Cancel an active session's streaming task.
    pub fn cancel_session(active: &Arc<ActiveSessions>, session_id: &str) -> Result<(), AppError> {
        match active.get(session_id) {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => Err(AppError::Validation(
                "session is not actively streaming".into(),
            )),
        }
    }

    /// Convert stored messages into the agent's `Message` format.
    ///
    /// For feat-009, content is always treated as `Content::Text` since
    /// tool execution (structured blocks) is not yet active.
    fn build_message_history(messages: &[crate::store::sessions::Message]) -> Vec<agent::Message> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role.as_str() {
                    "user" => agent::Role::User,
                    "assistant" => agent::Role::Assistant,
                    // Treat unknown roles as user (shouldn't happen in practice)
                    _ => agent::Role::User,
                };
                agent::Message {
                    role,
                    content: agent::Content::Text(m.content.clone()),
                }
            })
            .collect()
    }

    /// Resolve the model to use for an agent request.
    ///
    /// Priority: session.model → specialist.model → provider default_model → hardcoded fallback.
    fn resolve_model(
        session: &crate::store::sessions::Session,
        specialist: Option<&Specialist>,
        db: &Db,
    ) -> String {
        // 1. Session-level override
        if let Some(ref model) = session.model {
            if !model.is_empty() {
                return model.clone();
            }
        }

        // 2. Specialist-level override
        if let Some(s) = specialist {
            if let Some(ref model) = s.model {
                if !model.is_empty() {
                    return model.clone();
                }
            }
        }

        // 3. Provider default_model from config_json
        match ProviderStore::get_by_id(db, &session.provider_id) {
            Ok(provider) => {
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&provider.config_json)
                {
                    if let Some(model) = config.get("default_model").and_then(|v| v.as_str()) {
                        return model.to_string();
                    }
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "failed to load provider for model resolution, using fallback"
                );
            }
        }

        // 4. Hardcoded fallback
        FALLBACK_MODEL.to_string()
    }
}

/// Log an error, transition the session to error status.
fn abort_with_error(db: &Arc<Db>, session_id: &str, e: impl std::fmt::Display, msg: &str) {
    error!(session_id, error = %e, msg);
    let _ = SessionStore::update_status(db, session_id, "error");
}

/// The spawned task that streams the agent response and persists the result.
async fn run_prompt_task(
    db: Arc<Db>,
    registry: Arc<ProviderRegistry>,
    specialists: Arc<SpecialistRegistry>,
    active: Arc<ActiveSessions>,
    sse_manager: Arc<sse::SseManager>,
    tools: Arc<ToolRegistry>,
    session: crate::store::sessions::Session,
    cancel_token: CancellationToken,
) {
    let session_id = &session.id;

    // Ensure we always clean up the active session entry
    let _guard = SessionGuard {
        active: Arc::clone(&active),
        session_id: session_id.to_string(),
    };

    // Transition connecting -> ready if needed
    if session.status == "connecting" {
        if let Err(e) = SessionStore::update_status(&db, session_id, "ready") {
            abort_with_error(&db, session_id, e, "failed to transition session to ready");
            return;
        }
    }

    // Load message history
    let messages = match load_all_messages(&db, session_id) {
        Ok(msgs) => msgs,
        Err(e) => {
            abort_with_error(&db, session_id, e, "failed to load message history");
            return;
        }
    };
    let history = SessionService::build_message_history(&messages);

    // Resolve specialist (used for both model override and system prompt)
    let specialist =
        session
            .specialist_id
            .as_deref()
            .and_then(|id| match specialists.get_by_name(id) {
                Some(s) => Some(s),
                None => {
                    warn!(
                        session_id = session_id,
                        specialist_id = id,
                        "specialist not found in registry, proceeding without system prompt"
                    );
                    None
                }
            });

    // Resolve model — priority: session → specialist → provider → fallback
    let model = SessionService::resolve_model(&session, specialist, &db);

    // Resolve tools from specialist's tool profile
    let tool_defs = if let Some(s) = specialist {
        if let Some(ref profile) = s.tool_profile {
            match tools.resolve_profile(profile) {
                Ok(defs) if defs.is_empty() => None,
                Ok(defs) => Some(defs),
                Err(e) => {
                    abort_with_error(&db, session_id, e, "invalid tool profile");
                    return;
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // System prompt from specialist
    let system_prompt = specialist.map(|s| s.system_prompt.clone());

    // Get the agent from the registry
    let agent = match registry.get_agent(&session.provider_id) {
        Ok(a) => a,
        Err(e) => {
            abort_with_error(&db, session_id, e, "failed to get agent from registry");
            return;
        }
    };

    // Call the agent
    let stream = match agent
        .send_message(MessageRequest {
            model,
            messages: history,
            system: system_prompt,
            max_tokens: DEFAULT_MAX_TOKENS,
            tools: tool_defs,
        })
        .await
    {
        Ok(s) => s,
        Err(e) => {
            abort_with_error(&db, session_id, e, "agent send_message failed");
            return;
        }
    };

    // Stream and accumulate with cancellation support
    use futures_util::StreamExt;

    let mut accumulated = String::new();
    let mut stream = stream;
    let mut had_error = false;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "session cancelled by user");
                sse_manager.broadcast(
                    session_id,
                    SseWireEvent::Done { stop_reason: agent::StopReason::Cancelled },
                );
                let _ = SessionStore::update_status(&db, session_id, "cancelled");
                return;
            }
            item = StreamExt::next(&mut stream) => {
                match item {
                    Some(Ok(event)) => {
                        // Broadcast every agent event to SSE subscribers
                        let is_terminal = matches!(
                            event,
                            agent::StreamEvent::Done { .. } | agent::StreamEvent::Error { .. }
                        );
                        sse_manager.broadcast(session_id, sse::stream_event_to_wire(event.clone()));
                        match event {
                            agent::StreamEvent::TextDelta { text } => {
                                accumulated.push_str(&text);
                            }
                            agent::StreamEvent::Done { .. } => {
                                break;
                            }
                            agent::StreamEvent::Error { message } => {
                                error!(session_id, error = %message, "agent stream error");
                                had_error = true;
                                break;
                            }
                            _ => {}
                        }
                        if is_terminal {
                            break;
                        }
                    },
                    Some(Err(e)) => {
                        error!(session_id, error = %e, "agent stream provider error");
                        had_error = true;
                        sse_manager.broadcast(
                            session_id,
                            SseWireEvent::Error { message: e.to_string() },
                        );
                        break;
                    }
                    None => {
                        // Stream ended without a Done event
                        break;
                    }
                }
            }
        }
    }

    // Check cancellation after loop (race between Done and Cancel)
    if cancel_token.is_cancelled() {
        info!(session_id, "session cancelled by user (after stream end)");
        sse_manager.broadcast(
            session_id,
            SseWireEvent::Done {
                stop_reason: agent::StopReason::Cancelled,
            },
        );
        let _ = SessionStore::update_status(&db, session_id, "cancelled");
        return;
    }

    // Save assistant message if we have content
    if !accumulated.is_empty() {
        if let Err(e) = MessageStore::create(&db, session_id, "assistant", &accumulated, None) {
            abort_with_error(&db, session_id, e, "failed to save assistant message");
            return;
        }
    }

    // Update final session status
    let final_status = if had_error { "error" } else { "completed" };
    if let Err(e) = SessionStore::update_status(&db, session_id, final_status) {
        error!(session_id, error = %e, "failed to update session to {}", final_status);
    }
}

/// Load all messages for a session (paginated, up to MAX_HISTORY_MESSAGES).
fn load_all_messages(
    db: &Db,
    session_id: &str,
) -> Result<Vec<crate::store::sessions::Message>, AppError> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let page = MessageStore::list_by_session(db, session_id, cursor.as_deref(), 100)?;
        let has_more = page.cursor.is_some();
        cursor = page.cursor;
        all.extend(page.data);
        if !has_more || all.len() >= MAX_HISTORY_MESSAGES {
            break;
        }
    }

    all.truncate(MAX_HISTORY_MESSAGES);
    Ok(all)
}

/// Drop guard that ensures `active_sessions.remove()` is called even on panic.
struct SessionGuard {
    active: Arc<ActiveSessions>,
    session_id: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.active.remove(&self.session_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sessions::tests::seed_deps;
    use crate::tools::test_support::MockTool;
    use crate::tools::ToolRegistry;
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn test_db() -> Arc<Db> {
        Arc::new(Db::open(std::path::Path::new(":memory:")).unwrap())
    }

    /// A mock agent that captures the MessageRequest it receives.
    struct CapturingAgent {
        captured: Arc<Mutex<Option<MessageRequest>>>,
    }

    #[async_trait]
    impl crate::agent::CodingAgent for CapturingAgent {
        fn provider_type(&self) -> &str {
            "mock"
        }
        fn display_name(&self) -> &str {
            "Mock"
        }
        async fn list_models(
            &self,
        ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
            Ok(vec![])
        }
        async fn send_message(
            &self,
            request: MessageRequest,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures_core::Stream<
                            Item = Result<agent::StreamEvent, crate::error::ProviderError>,
                        > + Send,
                >,
            >,
            crate::error::ProviderError,
        > {
            *self.captured.lock().unwrap() = Some(request);
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(agent::StreamEvent::Done {
                        stop_reason: agent::StopReason::EndTurn,
                    }))
                    .await;
            });
            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }
        async fn health_check(
            &self,
        ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
            Ok(crate::agent::ProviderHealth {
                healthy: true,
                latency_ms: 0,
                error: None,
            })
        }
    }

    fn test_state() -> (
        Arc<Db>,
        Arc<ProviderRegistry>,
        Arc<SpecialistRegistry>,
        Arc<ActiveSessions>,
        Arc<crate::sse::SseManager>,
        Arc<ToolRegistry>,
    ) {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());
        (db, registry, specialists, active, sse, tools)
    }

    #[test]
    fn test_build_message_history_user_only() {
        let messages = vec![crate::store::sessions::Message {
            id: "1".into(),
            session_id: "s1".into(),
            role: "user".into(),
            content: "hello".into(),
            metadata: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        }];

        let history = SessionService::build_message_history(&messages);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, agent::Role::User);
        match &history[0].content {
            agent::Content::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected Content::Text"),
        }
    }

    #[test]
    fn test_build_message_history_mixed_roles() {
        let messages = vec![
            crate::store::sessions::Message {
                id: "1".into(),
                session_id: "s1".into(),
                role: "user".into(),
                content: "hi".into(),
                metadata: None,
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            crate::store::sessions::Message {
                id: "2".into(),
                session_id: "s1".into(),
                role: "assistant".into(),
                content: "hello!".into(),
                metadata: None,
                created_at: "2026-01-01T00:00:01Z".into(),
            },
        ];

        let history = SessionService::build_message_history(&messages);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, agent::Role::User);
        assert_eq!(history[1].role, agent::Role::Assistant);
    }

    #[test]
    fn test_resolve_model_session_override() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let (ws_id, provider_id) = seed_deps(&db);

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            Some("claude-opus-4-20250514"),
            None,
            None,
        )
        .unwrap();

        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, "claude-opus-4-20250514");
    }

    #[test]
    fn test_resolve_model_provider_default() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let (ws_id, provider_id) = seed_deps(&db);

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // seed_deps creates provider with default_model in config_json
        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_model_hardcoded_fallback() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();

        // Create a provider with no default_model in config
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "anthropic",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k"}"#,
        )
        .unwrap();

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, FALLBACK_MODEL);
    }

    #[test]
    fn test_active_sessions_try_insert() {
        let active = ActiveSessions::new();
        let token = CancellationToken::new();

        // First insert succeeds
        assert!(active.try_insert("s1".to_string(), token.clone()));
        assert!(active.contains("s1"));

        // Duplicate insert fails (TOCTOU-safe)
        assert!(!active.try_insert("s1".to_string(), CancellationToken::new()));

        // Remove and re-insert works
        active.remove("s1");
        assert!(!active.contains("s1"));
        assert!(active.try_insert("s1".to_string(), token));
    }

    #[tokio::test]
    async fn test_send_prompt_empty_prompt() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "",
        )
        .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn test_send_prompt_session_not_found() {
        let (db, registry, specialists, active, sse, tools) = test_state();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            "nonexistent",
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_send_prompt_terminal_session() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Transition to terminal state
        crate::store::sessions::SessionStore::update_status(&db, &session.id, "completed").unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn test_cancel_session_not_active() {
        let (_, _, _, active, _, _) = test_state();

        let result = SessionService::cancel_session(&active, "nonexistent");
        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn test_send_prompt_conflict_on_double_send() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Simulate an active session
        let token = CancellationToken::new();
        active.try_insert(session.id.clone(), token);

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_send_prompt_flow() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Send prompt — should succeed and return a message ID
        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        let message_id = result.unwrap();
        assert!(!message_id.is_empty());

        // User message should be saved with raw content (no JSON encoding)
        let messages =
            crate::store::sessions::MessageStore::list_by_session(&db, &session.id, None, 10)
                .unwrap();
        assert_eq!(messages.data.len(), 1);
        assert_eq!(messages.data[0].role, "user");
        assert_eq!(messages.data[0].content, "hello");
        assert_eq!(messages.data[0].id, message_id);
    }

    #[tokio::test]
    async fn test_cancel_session() {
        // Verifies the cancel API rejects non-active sessions
        let (_, _, _, active, _, _) = test_state();
        let result = SessionService::cancel_session(&active, "nonexistent");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_session_with_mock_agent() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        /// A mock agent that streams events with delays, allowing cancel to fire.
        struct SlowAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for SlowAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    // Send a text delta, then wait long enough for cancel to fire
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "partial".into(),
                        }))
                        .await;
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Done {
                            stop_reason: agent::StopReason::EndTurn,
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        // Create workspace, provider, and register the mock agent
        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(SlowAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Send prompt — the mock agent will stream slowly
        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());
        assert!(active.contains(&session.id));

        // Cancel — should succeed
        let result = SessionService::cancel_session(&active, &session.id);
        assert!(result.is_ok());

        // Wait for the spawned task to complete (poll up to 2 seconds)
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !active.contains(&session.id),
            "session should be removed from active set after cancel"
        );

        // Session status should be cancelled
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "cancelled");
    }

    #[tokio::test]
    async fn test_sse_broadcast_on_prompt() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        /// A mock agent that streams text deltas and a done event.
        struct MockStreamAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for MockStreamAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "Hello".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: " World".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Done {
                            stop_reason: agent::StopReason::EndTurn,
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(MockStreamAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Subscribe to SSE stream before sending prompt
        let mut rx = sse.subscribe(&session.id);

        // Send prompt
        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Collect SSE events until we see the Done event
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => {
                    let is_done = matches!(&event, crate::sse::SseWireEvent::Done { .. });
                    events.push(event);
                    if is_done {
                        break;
                    }
                }
                _ => break,
            }
        }

        // Verify: should have 2 TextDelta + 1 Done
        assert!(
            events.len() >= 3,
            "expected at least 3 events, got {}",
            events.len()
        );

        let text_deltas: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                crate::sse::SseWireEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["Hello", " World"]);

        let has_done = events.iter().any(|e| {
            matches!(
                e,
                crate::sse::SseWireEvent::Done {
                    stop_reason: agent::StopReason::EndTurn
                }
            )
        });
        assert!(has_done, "expected Done event with EndTurn");

        // Wait for task to finish
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Session should be completed
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "completed");
    }

    #[tokio::test]
    async fn test_specialist_system_prompt_injection() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let captured = Arc::new(Mutex::new(None));
        let registry = Arc::new(ProviderRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        // Build a specialist registry with one specialist
        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-injection");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("coder.md"),
            r#"---
name: coder
description: Writes code
model: claude-opus-4-20250514
---
You are a senior Rust engineer."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(
            &provider.id,
            Arc::new(CapturingAgent {
                captured: Arc::clone(&captured),
            }),
        );

        // Create session WITH specialist_id set
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            Some("coder"), // specialist_id
            None,
            None,
            None,
        )
        .unwrap();

        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "write a function",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Wait for the spawned task to complete
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Verify the captured request
        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("request was not captured");

        // System prompt should be set from specialist
        assert_eq!(
            request.system,
            Some("You are a senior Rust engineer.".to_string())
        );

        // Model should be overridden by specialist (claude-opus-4-20250514)
        assert_eq!(request.model, "claude-opus-4-20250514");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_tool_profile_filtering() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let captured = Arc::new(Mutex::new(None));
        let registry = Arc::new(ProviderRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());

        // Build a tool registry with tools matching the "planning" profile
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Arc::new(MockTool::new("get_task")));
        tool_registry.register(Arc::new(MockTool::new("list_tasks")));
        tool_registry.register(Arc::new(MockTool::new("update_task_fields")));
        tool_registry.register(Arc::new(MockTool::new("kanban")));
        tool_registry.register(Arc::new(MockTool::new("notes")));
        let tools = Arc::new(tool_registry);

        // Build a specialist registry with a specialist using "planning" profile
        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-tool-filtering");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("planner.md"),
            r#"---
name: planner
description: Plans tasks
tool_profile: planning
---
You are a planning specialist."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(
            &provider.id,
            Arc::new(CapturingAgent {
                captured: Arc::clone(&captured),
            }),
        );

        // Create session WITH specialist_id set to "planner"
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            Some("planner"),
            None,
            None,
            None,
        )
        .unwrap();

        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "plan my work",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Wait for the spawned task to complete
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Verify the captured request has the planning profile tools
        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("request was not captured");

        let tool_defs = request.tools.expect("expected tools to be set");
        let mut tool_names: Vec<&str> = tool_defs.iter().map(|d| d.name.as_str()).collect();
        tool_names.sort();
        assert_eq!(
            tool_names,
            vec![
                "get_task",
                "kanban",
                "list_tasks",
                "notes",
                "update_task_fields"
            ]
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_send_prompt_invalid_tool_profile() {
        // Create a specialist with an invalid tool_profile
        let (db, registry, _, active, sse, _) = test_state();
        let tools = {
            let mut tr = ToolRegistry::new();
            tr.register(Arc::new(MockTool::new("task")));
            Arc::new(tr)
        };

        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-invalid-profile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("broken.md"),
            r#"---
name: broken
description: Broken specialist
tool_profile: nonexistent_profile
---
You are broken."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            Some("broken"),
            None,
            None,
            None,
        )
        .unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("nonexistent_profile"));
            }
            other => panic!("expected Validation, got: {:?}", other),
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
