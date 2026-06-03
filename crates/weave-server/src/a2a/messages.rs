//! SendMessage handler — `POST /api/a2a/messages`.
//!
//! The primary A2A endpoint. Creates or continues a Weave session
//! based on the incoming A2A message, then sends the prompt
//! to the AI provider. Returns an A2A Task with the session ID.

use axum::{
    http::{HeaderMap, StatusCode},
    Extension, Json,
};

use super::auth::verify_a2a_token;
use super::types::*;
use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::service::sessions::SessionService;
use crate::store::providers::ProviderStore;
use crate::store::sessions::SessionStore;
use crate::store::workspaces::WorkspaceStore;
use crate::AppState;

/// `POST /api/a2a/messages`
///
/// Authenticated. Creates a new session (or continues an existing
/// one if `task_id` is provided), sends the extracted text as a
/// prompt, and returns the A2A Task.
pub async fn send_message(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    Json(body): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<DataResponse<Task>>), AppError> {
    // 1. Verify auth
    verify_a2a_token(&state.a2a_token, &headers)?;

    // 2. Extract text from message parts
    let prompt = extract_text_from_parts(&body.message.parts);
    if prompt.trim().is_empty() {
        return Err(AppError::Validation(
            "message must contain at least one text part with non-empty content".into(),
        ));
    }

    // 3. Resolve workspace — use default workspace
    WorkspaceStore::ensure_default(&state.db)?;
    let workspace_id = WorkspaceStore::get_default_id(&state.db)?;

    // 4. Resolve provider — first available
    let provider_id = first_provider_id(&state.db)?;

    // 5. Create or continue session
    let session = if let Some(ref task_id) = body.task_id {
        // Continue existing session — validate it exists and is non-terminal
        let session = SessionStore::get_by_id(&state.db, task_id)?;
        if crate::store::sessions::TERMINAL.contains(&session.status.as_str()) {
            return Err(AppError::Validation(format!(
                "cannot send message to task in terminal status '{}'",
                session.status
            )));
        }
        // Use the session's context_id if the request didn't provide one
        let _ctx_id = body.context_id.as_deref().or(session.context_id.as_deref());
        // Send prompt to existing session
        SessionService::send_prompt(
            &state.db,
            &state.registry,
            &state.specialists,
            &state.active_sessions,
            &state.sse_manager,
            &state.tools,
            &session.id,
            &prompt,
        )
        .await?;
        // Re-fetch to get updated status
        SessionStore::get_by_id(&state.db, &session.id)?
    } else {
        // Create new session
        crate::service::sessions::SessionService::create_session(
            &state.db,
            &workspace_id,
            &provider_id,
            None,                       // specialist_id — none for generic A2A
            None,                       // model — use provider default
            None,                       // cwd — no filesystem context
            None,                       // parent_session_id — fresh session
            body.context_id.as_deref(), // context_id — A2A task linking
        )?
    };

    // 6. Send prompt for new sessions (already sent for existing)
    if body.task_id.is_none() {
        SessionService::send_prompt(
            &state.db,
            &state.registry,
            &state.specialists,
            &state.active_sessions,
            &state.sse_manager,
            &state.tools,
            &session.id,
            &prompt,
        )
        .await?;
    }

    // 7. Build and return A2A task
    let task = Task {
        id: session.id,
        context_id: session.context_id,
        status: TaskStatus::from_session_status(&session.status),
        history: None,
        artifacts: None,
    };

    Ok((StatusCode::CREATED, Json(DataResponse { data: task })))
}

/// Return the ID of the first provider (by creation order).
///
/// Mirrors `first_provider_id` in `service/kanban.rs`.
fn first_provider_id(db: &crate::db::Db) -> Result<String, AppError> {
    let providers = ProviderStore::list(db)?;
    providers
        .first()
        .map(|p| p.id.clone())
        .ok_or_else(|| {
            AppError::Validation(
                "no AI provider configured; add a provider via POST /api/providers before sending A2A messages"
                    .into(),
            )
        })
}
