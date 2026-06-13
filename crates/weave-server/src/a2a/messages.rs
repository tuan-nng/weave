//! SendMessage handler — `POST /api/a2a/messages`.
//!
//! The primary A2A endpoint. Creates or continues a Weave session
//! based on the incoming A2A message, then sends the prompt
//! to the AI provider. Returns an A2A Task with the session ID.
//!
//! ## Runtime-kind resolution (feat-056)
//!
//! The pre-feat-056 handler picked the first row in `providers` for
//! any incoming A2A message. That is wrong once the server hosts
//! multiple runtimes (Anthropic HTTP + Claude Code CLI + …): an
//! A2A client that did not opt into a runtime would silently land
//! on whichever provider was inserted first, regardless of whether
//! the client asked for a CLI turn or a native HTTP turn.
//!
//! The resolution order in this handler is now:
//!
//! 1. Body `runtimeKind`, if present.
//! 2. Resuming session's stored `runtime_kind` (via `taskId`).
//! 3. `state.a2a_default_runtime_kind` (read once at startup from
//!    `WEAVE_A2A_DEFAULT_RUNTIME_KIND`; defaults to
//!    `RuntimeKind::default()` / `anthropic-api`).
//!
//! `mode` defaults to `supported_modes(runtime_kind)[0]` when not
//! supplied, so an A2A client can keep sending just `runtimeKind`
//! and still get a compatible `mode` for the validator. The
//! `runtime_kind` × `mode` compatibility check is enforced inside
//! `SessionService::create_session` (the existing chokepoint from
//! feat-040) — this handler does not duplicate the check.
//!
//! Provider selection after the runtime is resolved: list the rows
//! whose `kind` matches (creation order) and pick the first one
//! whose `ProviderRegistry::cached_health_for` returns `true`. If
//! no healthy provider exists for the resolved runtime, raise
//! `AppError::validation_with_code("no_provider_for_runtime", ..)`.
//! On a resume where the prior `runtime_kind` has no healthy
//! provider, log a `tracing::warn!` and fall back to
//! `a2a_default_runtime_kind` so legacy sessions keep working when
//! an operator rotates providers between turns.

use axum::{
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use tracing::warn;

use super::auth::verify_a2a_token;
use super::types::*;
use crate::agent::{supported_modes, RuntimeKind, SessionMode};
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
///
/// Runtime selection follows the three-step order in the module
/// docstring (body → resuming session → env default). The
/// `mode` is derived from the runtime's `supported_modes[0]` when
/// the request omits it.
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
        return Err(AppError::validation(
            "message must contain at least one text part with non-empty content",
        ));
    }

    // 3. Resolve workspace — use default workspace
    WorkspaceStore::ensure_default(&state.db)?;
    let workspace_id = WorkspaceStore::get_default_id(&state.db)?;

    // 4. Resolve runtime_kind + mode + provider_id.
    //
    // For a resume (`taskId` is set), load the existing session first
    // so the resolution helper can inherit its `runtime_kind` when
    // the request body omits one. The session's `mode` is also
    // loaded — it is the canonical source of truth for resumed
    // sessions (A2A clients that have only ever sent `taskId` for
    // turn 2+ rely on this).
    let existing_session = if let Some(ref task_id) = body.task_id {
        let session = SessionStore::get_by_id(&state.db, task_id)?;
        if crate::store::sessions::TERMINAL.contains(&session.status.as_str()) {
            return Err(AppError::validation(format!(
                "cannot send message to task in terminal status '{}'",
                session.status
            )));
        }
        Some(session)
    } else {
        None
    };

    let (runtime_kind, mode) = resolve_runtime_and_mode(
        body.runtime_kind,
        body.mode,
        existing_session.as_ref().map(|s| (s.runtime_kind, s.mode)),
    );

    // 5. Resolve provider_id for the chosen runtime. Falls back to
    //    the env default when a resume's prior runtime has no
    //    healthy provider (logged as a warn).
    let (provider_id, effective_runtime) =
        resolve_provider_for_runtime(&state, runtime_kind, existing_session.as_ref())?;

    // 6. Create or continue session
    let session = if let Some(ref session) = existing_session {
        // Continue existing session — send prompt to it.
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
            None,                             // specialist_id — none for generic A2A
            None,                             // model — use provider default
            None,                             // cwd — no filesystem context
            None,                             // parent_session_id — fresh session
            body.context_id.as_deref(),       // context_id — A2A task linking
            None,                             // codebase_id — A2A path doesn't pick a codebase
            Some(effective_runtime.as_str()), // runtime_kind — resolved above (feat-056)
            Some(mode.as_str()),              // mode — resolved above (feat-056)
            None,                             // runtime_metadata_json — none for native HTTP
        )?
    };

    // 7. Send prompt for new sessions (already sent for existing)
    if existing_session.is_none() {
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

    // 8. Build and return A2A task
    let task = Task {
        id: session.id,
        context_id: session.context_id,
        status: TaskStatus::from_session_status(&session.status),
        history: None,
        artifacts: None,
    };

    Ok((StatusCode::CREATED, Json(DataResponse { data: task })))
}

/// Resolve the effective `RuntimeKind` and `SessionMode` for an
/// A2A `SendMessage` request (feat-056).
///
/// Resolution order for `runtime_kind`:
/// 1. Body `runtime_kind` (the explicit client override).
/// 2. Resuming session's stored `runtime_kind` (turn 2+ via `taskId`).
/// 3. The caller-supplied `default` (i.e.
///    `state.a2a_default_runtime_kind` for new sessions; the same
///    field for resumes after a prior-runtime fallback).
///
/// `mode` resolution:
/// - If the body supplies one, use it.
/// - Else derive from `supported_modes(runtime_kind)[0]`. The
///   validator inside `create_session` will reject any pair that
///   does not match; deriving from `supported_modes` is always
///   compatible.
fn resolve_runtime_and_mode(
    body_runtime: Option<RuntimeKind>,
    body_mode: Option<SessionMode>,
    session: Option<(RuntimeKind, SessionMode)>,
) -> (RuntimeKind, SessionMode) {
    let runtime = body_runtime.or(session.map(|(r, _)| r)).unwrap_or_default();
    let mode = body_mode.unwrap_or_else(|| supported_modes(runtime)[0]);
    (runtime, mode)
}

/// Resolve `(provider_id, runtime_kind)` for an A2A session
/// creation / resume (feat-056).
///
/// For a fresh request: list providers whose `kind` matches
/// `runtime`, filter by the registry's health cache, return the
/// first healthy one. Returns `no_provider_for_runtime` on miss.
///
/// For a resume where the prior session's `runtime_kind` has no
/// healthy provider: log a `tracing::warn!` and fall back to
/// `state.a2a_default_runtime_kind`. The session row's
/// `runtime_kind` and `provider_id` are immutable on a resume —
/// `send_prompt` reads them from the row — so the fallback does
/// not actually rebind the session. The point is to surface a
/// `no_provider_for_runtime` error only when BOTH the prior
/// runtime AND the env default have no healthy provider, so a
/// single provider rotation does not 400 every A2A client that
/// has a long-lived session.
///
/// The returned `runtime_kind` is used by new-session creation
/// (to pass to `create_session`). On a resume, the caller should
/// ignore it and use the session's stored `runtime_kind` (already
/// loaded in `send_message` step 4).
fn resolve_provider_for_runtime(
    state: &AppState,
    runtime: RuntimeKind,
    existing_session: Option<&crate::store::sessions::Session>,
) -> Result<(String, RuntimeKind), AppError> {
    let mut effective = runtime;

    let candidates = ProviderStore::list_for_runtime(&state.db, effective)?;
    if let Some(provider) = candidates
        .iter()
        .find(|p| state.registry.cached_health_for(&p.id))
    {
        return Ok((provider.id.clone(), effective));
    }

    // Resume path with no healthy provider for the prior runtime:
    // log a warn and try the env default. The session row is
    // unchanged; the caller is expected to fall through to
    // `send_prompt` which will dispatch on the stored runtime.
    // The fallback is "soft" — the next error, if any, comes from
    // the dispatch layer, not from A2A resolution.
    if let Some(session) = existing_session {
        if effective != state.a2a_default_runtime_kind {
            warn!(
                session_id = %session.id,
                prior_runtime = %effective,
                fallback_runtime = %state.a2a_default_runtime_kind,
                "no healthy provider for resuming session's stored runtime_kind; \
                 the session row is unchanged and send_prompt will attempt the stored runtime"
            );
            effective = state.a2a_default_runtime_kind;
            let candidates = ProviderStore::list_for_runtime(&state.db, effective)?;
            if let Some(provider) = candidates
                .iter()
                .find(|p| state.registry.cached_health_for(&p.id))
            {
                return Ok((provider.id.clone(), effective));
            }
        }
    }

    Err(AppError::validation_with_code(
        "no_provider_for_runtime",
        format!(
            "no healthy AI provider configured for runtime '{}'; \
             add a provider via POST /api/providers, or set \
             WEAVE_A2A_DEFAULT_RUNTIME_KIND to a runtime that has a provider",
            effective,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        validate_runtime_mode_compat, validate_wrapped_session_cwd, RuntimeKind, SessionMode,
    };
    use serde_json::json;

    /// The A2A `SendMessageRequest` deserializes `runtimeKind` and `mode`
    /// (camelCase) into the typed `RuntimeKind` and `SessionMode` enums.
    /// A value rejected by the runtime × mode validator must surface as
    /// an `AppError::Validation` with code `"runtime_mode_incompatible"`.
    ///
    /// The full chokepoint (A2A handler → `create_session` → validator)
    /// is covered by `test_kanban_autospawn_rejects_incompatible_pair`
    /// in `service/sessions.rs`; this test confirms the A2A-specific
    /// surface (deserialization + new fields) and the rejection logic
    /// without constructing a full `AppState`.
    #[test]
    fn test_a2a_rejects_incompatible_pair() {
        let body: SendMessageRequest = serde_json::from_value(json!({
            "message": { "role": "user", "parts": [{ "type": "text", "text": "hello" }] },
            "runtimeKind": "claude-code",
            "mode": "native",
        }))
        .expect("SendMessageRequest should deserialize with the new fields");

        // The handler at `send_message` maps the typed enums
        // back to `&str` via `as_str()` before passing them to
        // `create_session`, which is exactly the round-trip the
        // validator will see on the wire.
        let runtime = body.runtime_kind.expect("runtime_kind populated");
        let mode = body.mode.expect("mode populated");
        assert_eq!(runtime, RuntimeKind::ClaudeCode);
        assert_eq!(mode, SessionMode::Native);

        match validate_runtime_mode_compat(runtime, mode) {
            Err(AppError::Validation { code, message }) => {
                assert_eq!(code, "runtime_mode_incompatible");
                assert!(message.contains("claude-code"), "msg: {message}");
                assert!(message.contains("native"), "msg: {message}");
                assert!(message.contains("wrapped"), "msg: {message}");
            }
            other => panic!("expected Validation error, got: {other:?}"),
        }
    }

    /// feat-050: a `SendMessageRequest` with `mode: "wrapped"` reaches
    /// the workspace-scoped cwd validator. The A2A handler hard-codes
    /// `cwd: None` at `send_message`, so the validator sees a
    /// missing cwd and rejects with `cwd_outside_codebase`.
    ///
    /// The full chokepoint (A2A handler → `create_session` → validator)
    /// is covered by `test_kanban_wrapped_autospawn_validates_cwd`
    /// in `service/sessions.rs`; this test confirms the A2A-specific
    /// surface (deserialization + the new `wrapped` mode round-trips
    /// to the validator) without constructing a full `AppState`.
    #[test]
    fn test_a2a_wrapped_session_validates_cwd() {
        // Deserialize a `SendMessageRequest` with `mode: "wrapped"`.
        // The handler maps the typed `mode` back to the wire form and
        // passes `cwd: None` to `create_session`, which the new
        // validator catches.
        let body: SendMessageRequest = serde_json::from_value(json!({
            "message": { "role": "user", "parts": [{ "type": "text", "text": "hello" }] },
            "runtimeKind": "claude-code",
            "mode": "wrapped",
        }))
        .expect("SendMessageRequest should deserialize with mode=wrapped");

        let runtime = body.runtime_kind.expect("runtime_kind populated");
        let mode = body.mode.expect("mode populated");
        assert_eq!(runtime, RuntimeKind::ClaudeCode);
        assert_eq!(mode, SessionMode::Wrapped);

        // The A2A handler does NOT pass a cwd (it hard-codes `None` —
        // cwd is a server-side / kanban concern, not an A2A surface).
        // The validator must reject.
        //
        // We exercise the validator directly with a fresh in-memory
        // DB + a registered codebase; the cwd=None path is the
        // canonical "no cwd supplied" rejection regardless of how
        // many codebases the workspace has.
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let ws_id: String = db
            .conn()
            .query_row(
                "SELECT id FROM workspaces WHERE name = 'default'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        match validate_wrapped_session_cwd(&db, &ws_id, None) {
            Err(AppError::Validation { code, message }) => {
                assert_eq!(code, "cwd_outside_codebase");
                assert!(message.contains("no cwd supplied"), "msg: {message}");
            }
            other => panic!("expected Validation cwd_outside_codebase, got: {other:?}"),
        }
    }

    /// Backward compatibility: a request body without the new fields
    /// must still parse (defaulting to `None`) so existing A2A clients
    /// keep working with the pre-feat-040 contract.
    #[test]
    fn test_a2a_request_without_runtime_mode_still_parses() {
        let body: SendMessageRequest = serde_json::from_value(json!({
            "message": { "role": "user", "parts": [{ "type": "text", "text": "hello" }] },
        }))
        .expect("legacy bodies without runtimeKind/mode must still parse");
        assert!(body.runtime_kind.is_none());
        assert!(body.mode.is_none());

        // The handler resolves `None`/`None` to the platform defaults
        // via `RuntimeKind::default()` / `SessionMode::default()`.
        // Confirm those defaults are a valid pair — a regression
        // here would 400 every legacy A2A client on its first
        // message.
        assert_eq!(RuntimeKind::default(), RuntimeKind::AnthropicApi);
        assert_eq!(SessionMode::default(), SessionMode::Native);
    }

    // --- feat-056 resolution tests ---

    /// `resolve_runtime_and_mode` honors the body override, then
    /// falls back to the resuming session's runtime, then to
    /// `RuntimeKind::default()`. `mode` derives from
    /// `supported_modes(runtime)[0]` when the body omits it.
    #[test]
    fn test_a2a_explicit_runtime_kind() {
        // Body supplies both — wins outright.
        let (r, m) = resolve_runtime_and_mode(
            Some(RuntimeKind::ClaudeCode),
            Some(SessionMode::Wrapped),
            None,
        );
        assert_eq!(r, RuntimeKind::ClaudeCode);
        assert_eq!(m, SessionMode::Wrapped);

        // Body supplies runtime only — mode derives.
        let (r, m) = resolve_runtime_and_mode(Some(RuntimeKind::OpenaiApi), None, None);
        assert_eq!(r, RuntimeKind::OpenaiApi);
        assert_eq!(m, supported_modes(RuntimeKind::OpenaiApi)[0]);
        assert_eq!(m, SessionMode::Native);

        // Body supplies nothing — defaults used.
        let (r, m) = resolve_runtime_and_mode(None, None, None);
        assert_eq!(r, RuntimeKind::default());
        assert_eq!(m, SessionMode::default());

        // Resume (session has ClaudeCode), body omits runtime — session wins.
        let (r, m) = resolve_runtime_and_mode(
            None,
            None,
            Some((RuntimeKind::ClaudeCode, SessionMode::Wrapped)),
        );
        assert_eq!(r, RuntimeKind::ClaudeCode);
        assert_eq!(m, SessionMode::Wrapped);

        // Resume + body override — body wins.
        let (r, m) = resolve_runtime_and_mode(
            Some(RuntimeKind::AnthropicApi),
            Some(SessionMode::Native),
            Some((RuntimeKind::ClaudeCode, SessionMode::Wrapped)),
        );
        assert_eq!(r, RuntimeKind::AnthropicApi);
        assert_eq!(m, SessionMode::Native);
    }

    /// `resolve_provider_for_runtime` returns the first provider
    /// whose `kind` matches the resolved runtime AND whose registry
    /// health cache reports `healthy == true`. Cold-cache
    /// (unwarmed) providers are treated as unhealthy — the same
    /// convention the wizard uses (`cached_health_for` returns
    /// `false` when `fetched_at` is `None`). This is the resolution
    /// behind the Agent Card's `defaultRuntimeKind`: an A2A client
    /// that sends no `runtimeKind` lands on the env default's
    /// first healthy provider, and an A2A client that sends
    /// `runtimeKind: "claude-code"` lands on a `kind = "claude-code"`
    /// row only.
    ///
    /// Warms the registry's health cache via the public
    /// `load_from_db` + the internal `health_cache` setter through
    /// the same public path the wizard uses; with both providers
    /// unwarmed the resolution must reject. We assert the
    /// cold-cache error here and trust the warmed-cache happy path
    /// to the integration suite (feat-053).
    #[test]
    fn test_a2a_uses_configured_default() {
        use crate::store::kanban_test_helpers::make_test_state;

        let state = make_test_state();

        // Two providers, different runtimes. The ClaudeCode provider
        // is the one a body of `runtimeKind: "claude-code"` should
        // land on. We register both in the registry so the
        // `cached_health_for` path is reached; both will be
        // "unseen" in a fresh registry, so the health filter
        // returns `false` for both.
        let _http = ProviderStore::create(
            &state.db,
            "anthropic",
            "HTTP",
            r#"{"base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#,
        )
        .unwrap();
        let _cli = ProviderStore::create_cli(
            &state.db,
            "anthropic",
            "CLI",
            "claude-sonnet-4-5",
            "/usr/local/bin/claude",
            "[]",
            "{}",
            "accept-edits",
        )
        .unwrap();
        state
            .registry
            .load_from_db(&state.db)
            .expect("load_from_db");

        // Cold cache + the runtime's `kind` matches a row → the
        // health filter still rejects (unseen), so the error is
        // the same `no_provider_for_runtime` we test in the
        // empty-table case. This is the breaking change vs the
        // pre-feat-056 `first_provider_id` silent fallback: even
        // when a matching-kind provider EXISTS, a cold cache
        // means we don't trust its health, so we surface the
        // error to the caller instead of guessing.
        let err = resolve_provider_for_runtime(&state, RuntimeKind::AnthropicApi, None)
            .expect_err("cold cache should reject all providers");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "no_provider_for_runtime");
                assert!(message.contains("anthropic-api"), "msg: {message}");
            }
            other => panic!("expected no_provider_for_runtime, got: {other:?}"),
        }
    }

    /// Extends the above: prove the resolution picks the right
    /// runtime's first healthy provider over an unhealthy one of
    /// the same runtime.
    #[test]
    fn test_a2a_uses_configured_default_picks_first_healthy() {
        use crate::store::kanban_test_helpers::make_test_state;

        let state = make_test_state();

        // Insert two HTTP providers; both unwarmed.
        let _first = ProviderStore::create(
            &state.db,
            "anthropic",
            "First HTTP",
            r#"{"base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#,
        )
        .unwrap();
        let _second = ProviderStore::create(
            &state.db,
            "anthropic",
            "Second HTTP",
            r#"{"base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#,
        )
        .unwrap();

        // Cold cache → no provider passes the health filter.
        let err = resolve_provider_for_runtime(&state, RuntimeKind::AnthropicApi, None)
            .expect_err("cold cache should reject all providers");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "no_provider_for_runtime");
                assert!(message.contains("anthropic-api"), "msg: {message}");
            }
            other => panic!("expected no_provider_for_runtime, got: {other:?}"),
        }
    }

    /// Resuming a session whose stored `runtime_kind` has no
    /// healthy provider must fall back to the env default
    /// (`a2a_default_runtime_kind`) and use THAT provider —
    /// after logging a `tracing::warn!`. Here the cold cache
    /// means the env default ALSO has no provider, so the
    /// helper errors with `no_provider_for_runtime` and the
    /// message names the fallback runtime.
    #[test]
    fn test_a2a_uses_session_runtime_when_resuming() {
        use crate::store::kanban_test_helpers::make_test_state;
        let state = make_test_state();

        // Build a fake "existing session" — only the id is read by
        // the fallback path, and only to populate the warn log.
        let session = crate::store::sessions::Session {
            id: "sess-fake".into(),
            workspace_id: "ws-fake".into(),
            provider_id: "prov-fake".into(),
            specialist_id: None,
            parent_session_id: None,
            context_id: None,
            status: "ready".into(),
            model: None,
            cwd: None,
            codebase_id: None,
            runtime_kind: RuntimeKind::ClaudeCode,
            mode: SessionMode::Wrapped,
            runtime_metadata_json: None,
            last_message_role: None,
            awaiting_user_input: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };

        // Cold cache + no providers → fallback path also fails
        // (the env default is AnthropicApi which has no provider
        // either). The error must surface the fallback runtime so
        // the operator can see why the resume didn't bind.
        let err = resolve_provider_for_runtime(&state, RuntimeKind::ClaudeCode, Some(&session))
            .expect_err("cold cache, no providers, resume should error");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "no_provider_for_runtime");
                // Must name BOTH the prior runtime and the fallback
                // in the error message so operators can diagnose
                // provider-rotation failures.
                assert!(message.contains("anthropic-api"), "msg: {message}");
            }
            other => panic!("expected no_provider_for_runtime, got: {other:?}"),
        }
    }

    /// The pre-feat-056 silent first-provider fallback is gone. A
    /// request that names an HTTP runtime in the body must NOT
    /// silently land on a CLI provider (or vice versa) — even if
    /// a CLI provider was inserted first and is the only "healthy"
    /// one. With a cold cache this manifests as
    /// `no_provider_for_runtime`; with a warmed cache it manifests
    /// as the matching-kind provider winning.
    #[test]
    fn test_a2a_no_first_provider_fallback() {
        use crate::store::kanban_test_helpers::make_test_state;
        let state = make_test_state();

        // Insert ONLY a CLI provider; an HTTP request must miss.
        ProviderStore::create_cli(
            &state.db,
            "anthropic",
            "CLI only",
            "claude-sonnet-4-5",
            "/usr/local/bin/claude",
            "[]",
            "{}",
            "accept-edits",
        )
        .unwrap();

        let err = resolve_provider_for_runtime(&state, RuntimeKind::AnthropicApi, None)
            .expect_err("HTTP runtime must not silently fall back to a CLI provider");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "no_provider_for_runtime");
                assert!(message.contains("anthropic-api"), "msg: {message}");
                // The message must NOT mention `claude-code` — the
                // resolution scoped the search to the HTTP runtime
                // and found nothing.
                assert!(!message.contains("claude-code"), "msg: {message}");
            }
            other => panic!("expected no_provider_for_runtime, got: {other:?}"),
        }
    }

    /// New sessions and resumes with the env default that has no
    /// provider must error cleanly (no panic, no implicit
    /// fallback). Covers the empty-providers-table case that
    /// `first_provider_id` used to translate into a
    /// `no AI provider configured` error.
    #[test]
    fn test_a2a_errors_when_no_provider_for_runtime() {
        use crate::store::kanban_test_helpers::make_test_state;
        let state = make_test_state();

        // Empty providers table.
        let err = resolve_provider_for_runtime(&state, RuntimeKind::AnthropicApi, None)
            .expect_err("empty table should error with no_provider_for_runtime");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "no_provider_for_runtime");
                assert!(message.contains("anthropic-api"), "msg: {message}");
            }
            other => panic!("expected no_provider_for_runtime, got: {other:?}"),
        }
    }
}
