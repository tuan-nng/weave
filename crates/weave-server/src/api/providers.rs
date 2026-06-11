use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use crate::agent::model_cache::list_cli_models_via_shell;
use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::providers::{Provider, ProviderStore};
use crate::AppState;

const MAX_NAME_LEN: usize = 100;

/// Request body for `POST /api/providers`.
///
/// feat-039 widens this to a discriminated union on `kind` (default
/// `"http"` for back-compat with pre-feat-039 callers that omit it).
/// All kind-specific fields are `Option`; the handler validates the
/// per-kind invariants before persisting.
#[derive(Deserialize)]
pub struct CreateProviderRequest {
    /// `"http"` (default if missing) or `"cli"`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub name: String,
    // HTTP-only fields.
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    // Common to both kinds (HTTP, CLI).
    pub default_model: Option<String>,
    // CLI-only fields.
    pub binary_path: Option<String>,
    pub args_json: Option<String>,
    pub env_json: Option<String>,
    pub permission_mode: Option<String>,
}

/// GET /api/providers
///
/// feat-053: enriches each row with `healthy: bool` from the registry's
/// 10s `HealthCache` so the new-session wizard's Step 1 can gate on
/// per-provider health without an extra round-trip. The `cached_health_for`
/// accessor is sync and lock-only (no I/O), so the handler stays `async`
/// without a real await — the signature is fixed by Axum and the cost is
/// negligible.
pub async fn list_providers(
    axum::Extension(state): axum::Extension<AppState>,
) -> Result<Json<DataResponse<Vec<Provider>>>, AppError> {
    let mut providers = ProviderStore::list(&state.db)?;
    for p in providers.iter_mut() {
        p.healthy = state.registry.cached_health_for(&p.id);
    }
    Ok(Json(DataResponse { data: providers }))
}

/// POST /api/providers
///
/// Validates the discriminated union by `kind`:
///   * `kind="http"` requires `base_url`, `api_key`, `default_model`
///     and rejects any CLI fields. Calls `ProviderStore::create` and
///     registers an `AnthropicAgent` in the runtime registry.
///   * `kind="cli"` requires `default_model`, `binary_path`,
///     `permission_mode`; validates `args_json` is a JSON array of
///     strings and `env_json` is a JSON object of string→string; rejects
///     `base_url`/`api_key`. Calls `ProviderStore::create_cli` and
///     does NOT register an agent (the dispatch adapter lands in
///     feat-051; the row is pre-registered without one).
pub async fn create_provider(
    axum::Extension(state): axum::Extension<AppState>,
    Json(body): Json<CreateProviderRequest>,
) -> Result<(StatusCode, Json<DataResponse<Provider>>), AppError> {
    // Validate name
    let name = body.name.trim();
    validate_name(name)?;

    // Validate provider type
    if body.provider_type != "anthropic" {
        return Err(AppError::validation_with_code(
            "unsupported_provider_type",
            format!(
                "unsupported provider type: '{}' (only 'anthropic' supported in v1)",
                body.provider_type
            ),
        ));
    }

    // Default kind = "http" for back-compat with pre-feat-039 callers.
    let kind = body.kind.as_deref().unwrap_or("http");

    match kind {
        "http" => create_http_provider(&state, &body, name),
        "cli" => create_cli_provider(&state, &body, name),
        other => Err(AppError::validation_with_code(
            "invalid_kind",
            format!("unsupported kind: '{other}' (only 'http' and 'cli' supported)"),
        )),
    }
}

/// Handle `kind="http"` — pre-existing path with one added field requirement.
fn create_http_provider(
    state: &AppState,
    body: &CreateProviderRequest,
    name: &str,
) -> Result<(StatusCode, Json<DataResponse<Provider>>), AppError> {
    let base_url = body.base_url.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "base_url is required for kind=http")
    })?;
    let api_key = body.api_key.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "api_key is required for kind=http")
    })?;
    let default_model = body.default_model.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "default_model is required for kind=http")
    })?;

    if base_url.trim().is_empty() {
        return Err(AppError::validation("base_url must not be empty"));
    }
    if api_key.trim().is_empty() {
        return Err(AppError::validation("api_key must not be empty"));
    }
    if default_model.trim().is_empty() {
        return Err(AppError::validation("default_model must not be empty"));
    }

    // CLI fields must not be set on HTTP rows.
    if body.binary_path.is_some()
        || body.args_json.is_some()
        || body.env_json.is_some()
        || body.permission_mode.is_some()
    {
        return Err(AppError::validation_with_code(
            "invalid_field",
            "binary_path/args_json/env_json/permission_mode must not be set for kind=http",
        ));
    }

    let config_json = serde_json::json!({
        "base_url": base_url.trim(),
        "api_key": api_key.trim(),
        "default_model": default_model.trim(),
    })
    .to_string();

    // Validate that the agent can be constructed (catches malformed config
    // at create time rather than at first dispatch).
    let agent =
        crate::agent::registry::ProviderRegistry::create_agent(&body.provider_type, &config_json)
            .map_err(|e| AppError::validation(format!("invalid provider config: {e}")))?;

    // Persist to DB
    let provider = ProviderStore::create(&state.db, &body.provider_type, name, &config_json)?;

    // Register agent in runtime registry
    state.registry.add_agent(&provider.id, agent);

    Ok((StatusCode::CREATED, Json(DataResponse { data: provider })))
}

/// Env-key denylist for CLI providers (feat-051, was promised in
/// `model_cache.rs:143-148`).
///
/// The CLI subprocess inherits its environment from us. Keys in this
/// set would let a row caller hijack the dynamic linker, replace
/// `PATH`, or otherwise escape the Weave process's trust boundary —
/// none are ever useful for a Claude Code / Codex / OpenCode call,
/// so the request is rejected with a 400.
///
/// Matching is case-insensitive (the C runtime normalizes env keys
/// on Linux/macOS, so `ld_preload` and `LD_PRELOAD` are the same
/// key to the child). The set is deliberately small and auditable;
/// `LD_*` and `DYLD_*` are the entire families the dynamic linker
/// honors, and `PATH` is the only one that matters for shell
/// resolution.
const DANGEROUS_ENV_KEYS: &[&str] = &[
    // Linux/BSD dynamic linker
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_BIND_NOW",
    "LD_DEBUG",
    "LD_DEBUG_OUTPUT",
    "LD_PROFILE",
    "LD_PROFILE_OUTPUT",
    "LD_SHOW_AUXV",
    "LD_USE_LOAD_BIAS",
    // macOS dynamic linker
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    // Shell / loader
    "PATH",
    "LD_PRELOAD_ARCH",
];

/// Reject a `env_json` map containing any denylisted key. Returns
/// `Err(AppError::validation_with_code("invalid_field", ...))` on the
/// first match. The handler surfaces this as a 400.
fn validate_env_keys(env: &BTreeMap<String, String>) -> Result<(), AppError> {
    for key in env.keys() {
        if DANGEROUS_ENV_KEYS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(key))
        {
            return Err(AppError::validation_with_code(
                "invalid_field",
                format!(
                    "env_json key '{key}' is not allowed for CLI providers \
                     (dynamic-linker / shell-loader key; see DANGEROUS_ENV_KEYS \
                     in api/providers.rs)"
                ),
            ));
        }
    }
    Ok(())
}

/// Handle `kind="cli"` — pre-register a CLI provider. No agent registration
/// in this slice; the dispatch adapter lands in feat-051.
fn create_cli_provider(
    state: &AppState,
    body: &CreateProviderRequest,
    name: &str,
) -> Result<(StatusCode, Json<DataResponse<Provider>>), AppError> {
    let default_model = body.default_model.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "default_model is required for kind=cli")
    })?;
    let binary_path = body.binary_path.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "binary_path is required for kind=cli")
    })?;
    let permission_mode = body.permission_mode.as_deref().ok_or_else(|| {
        AppError::validation_with_code("missing_field", "permission_mode is required for kind=cli")
    })?;

    if default_model.trim().is_empty() {
        return Err(AppError::validation("default_model must not be empty"));
    }
    let binary_path_trimmed = binary_path.trim();
    if binary_path_trimmed.is_empty() {
        return Err(AppError::validation("binary_path must not be empty"));
    }
    // The shell-out in `list_cli_models_via_shell` invokes
    // `Command::new(binary)` with no `current_dir`, so a relative
    // path would resolve against the server's cwd — surprising and
    // not what an operator expects. Require an absolute path so the
    // row is unambiguous regardless of where the server was started.
    if !FsPath::new(binary_path_trimmed).is_absolute() {
        return Err(AppError::validation_with_code(
            "invalid_field",
            format!("binary_path must be an absolute path: '{binary_path_trimmed}'"),
        ));
    }
    if permission_mode.trim().is_empty() {
        return Err(AppError::validation("permission_mode must not be empty"));
    }

    // Validate args_json: JSON array of strings. Default to "[]" if absent.
    let args_json = body.args_json.as_deref().unwrap_or("[]");
    let parsed_args: Vec<String> = serde_json::from_str(args_json).map_err(|e| {
        AppError::validation(format!("args_json must be a JSON array of strings: {e}"))
    })?;

    // Validate env_json: JSON object of string→string. Default to "{}" if absent.
    let env_json = body.env_json.as_deref().unwrap_or("{}");
    let parsed_env: BTreeMap<String, String> = serde_json::from_str(env_json).map_err(|e| {
        AppError::validation(format!(
            "env_json must be a JSON object of string→string: {e}"
        ))
    })?;
    // feat-051: reject dynamic-linker / shell-loader keys so a
    // malicious row can't hijack the child process. The set is in
    // `validate_env_keys` above and matches the promise in
    // `model_cache.rs:143-148`.
    validate_env_keys(&parsed_env)?;

    // Re-serialize the validated JSON so the row stores canonicalized
    // output (no trailing whitespace, no escaped key order surprises).
    let args_json_canonical = serde_json::to_string(&parsed_args).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("failed to canonicalize args_json: {e}"))
    })?;
    let env_json_canonical = serde_json::to_string(&parsed_env)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("failed to canonicalize env_json: {e}")))?;

    // HTTP fields must not be set on CLI rows.
    if body.base_url.is_some() || body.api_key.is_some() {
        return Err(AppError::validation_with_code(
            "invalid_field",
            "base_url/api_key must not be set for kind=cli",
        ));
    }

    // feat-051: basename allowlist gate. We persist the row, but only
    // if the basename is in the registered dispatcher set. The
    // v1 set is `claude` (real Claude Code) and `fake_cli` (the
    // conformance harness); future runtimes (Codex, OpenCode) add
    // their basenames to this match when their adapters land.
    //
    // The check is at request time so the row never reaches
    // `list_cli_models_via_shell` (which would otherwise shell out
    // to an arbitrary user-supplied binary with arbitrary
    // `args_json`).
    let basename = std::path::Path::new(binary_path_trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !matches!(basename, "claude" | "fake_cli") {
        return Err(AppError::validation_with_code(
            "invalid_field",
            format!(
                "binary_path basename '{basename}' is not a registered CLI dispatcher; \
                 feat-051 supports 'claude' (real Claude Code) and 'fake_cli' (conformance \
                 harness). Future runtimes (Codex, OpenCode) extend the allowlist."
            ),
        ));
    }

    let provider = ProviderStore::create_cli(
        &state.db,
        &body.provider_type,
        name,
        default_model.trim(),
        binary_path_trimmed,
        &args_json_canonical,
        &env_json_canonical,
        permission_mode.trim(),
    )?;

    // feat-051: register a `ClaudeCodeCodingAgent` in the runtime
    // registry. The dispatcher is by binary basename (per the
    // spec's "matching is by binary basename for now" rule):
    //   * `claude`     → real Claude Code
    //   * `fake_cli`   → test harness (conformance suite)
    // The runner is built with the AppState's SHARED
    // `ActiveChildProcesses` table so the HTTP cancel handler
    // and the cold-start reaper (feat-049) reach the same pid
    // table the runner writes to.
    let agent = build_claude_code_agent(state, &provider);
    if let Some(agent) = agent {
        state.registry.add_agent(&provider.id, agent);
    }

    Ok((StatusCode::CREATED, Json(DataResponse { data: provider })))
}

/// Build a `ClaudeCodeCodingAgent` from a freshly-created CLI
/// provider row (feat-051). Returns `None` when the row's
/// `binary_path` basename has no registered adapter (the row
/// still gets persisted; this just means no agent is registered
/// in the runtime).
fn build_claude_code_agent(
    state: &crate::AppState,
    provider: &Provider,
) -> Option<Arc<dyn crate::agent::CodingAgent>> {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::agent::claude_code::ClaudeCodeCodingAgent;

    let binary_path_str = provider.binary_path.as_deref()?;
    let basename = std::path::Path::new(binary_path_str)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !matches!(basename, "claude" | "fake_cli") {
        return None;
    }
    let binary_path = PathBuf::from(binary_path_str);
    let args_str = provider.args_json.as_deref().unwrap_or("[]");
    let args: Vec<String> = match serde_json::from_str(args_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                provider_id = %provider.id,
                error = %e,
                "create_cli_provider: invalid args_json; skipping agent registration"
            );
            return None;
        }
    };
    let env_str = provider.env_json.as_deref().unwrap_or("{}");
    let env: BTreeMap<String, String> = match serde_json::from_str(env_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                provider_id = %provider.id,
                error = %e,
                "create_cli_provider: invalid env_json; skipping agent registration"
            );
            return None;
        }
    };
    let default_model = provider
        .default_model
        .clone()
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
    let permission_mode = provider
        .permission_mode
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let agent = ClaudeCodeCodingAgent::new(
        binary_path,
        args,
        env,
        default_model,
        permission_mode,
        // Share the AppState-level pid table. The HTTP cancel
        // handler reads it; the cold-start reaper reads /proc
        // independently but uses the same table to deduplicate.
        state.registry.active_child_processes(),
        // Share the registry's `turn_outcomes` map. The agent's
        // spawn task writes into it; `run_prompt_task` reads
        // back via `take_turn_outcome`.
        state.registry.turn_outcomes_arc(),
    );
    Some(Arc::new(agent) as Arc<dyn crate::agent::CodingAgent>)
}

/// DELETE /api/providers/{id}
pub async fn delete_provider(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    // Verify provider exists
    ProviderStore::get_by_id(&state.db, &id)?;

    // Check for referencing sessions
    if ProviderStore::has_sessions(&state.db, &id)? {
        return Err(AppError::Conflict(format!(
            "provider {id} has associated sessions and cannot be deleted"
        )));
    }

    // Remove from DB and registry
    ProviderStore::delete(&state.db, &id)?;
    state.registry.remove_agent(&id);

    Ok(Json(DataResponse { data: () }))
}

/// GET /api/providers/{id}/models
///
/// Cache-aware model list. On a hit (any age), returns the cached
/// `Vec<ModelInfo>` without invoking the underlying agent or shelling
/// out. On a miss, fetches fresh and stores the result. `kind="http"`
/// rows fetch via the registered agent (the `AnthropicAgent` returns
/// `Ok(vec![])` today — the cache makes the stub cheap to call
/// repeatedly). `kind="cli"` rows shell out to the registered binary
/// with `<binary_path> <args_json...> --list-models` and parse the
/// JSON stdout.
pub async fn list_provider_models(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<Vec<crate::agent::ModelInfo>>>, AppError> {
    let provider = ProviderStore::get_by_id(&state.db, &id)?;

    // Cache hit (fresh or stale) short-circuits both code paths — the
    // public surface returns the same `Vec<ModelInfo>` either way; the
    // freshness flag is internal to the cache.
    if let Some((models, _fresh)) = state.registry.get_cached_models(&id) {
        return Ok(Json(DataResponse { data: models }));
    }

    let models = match provider.kind.as_str() {
        "http" => {
            let agent = state.registry.get_agent(&id)?;
            agent.list_models().await?
        }
        "cli" => list_cli_models_via_shell(&provider).await?,
        other => {
            return Err(AppError::validation_with_code(
                "invalid_kind",
                format!("unsupported kind: '{other}' (only 'http' and 'cli' supported)"),
            ));
        }
    };

    state.registry.put_cached_models(&id, models.clone());
    Ok(Json(DataResponse { data: models }))
}

/// POST /api/providers/{id}/models — force-refresh the model-cache
/// entry for `provider_id`. Rejects `kind="http"` rows with a 400
/// (`AppError::validation_with_code("invalid_kind", ...)`) so the API
/// is symmetric: there is no "refresh" path for HTTP runtimes because
/// the registry's agent is the single source of truth, and the agent's
/// `list_models` is cheap to call (Anthropic's HTTP call is the
/// expensive side, and that lives outside this slice).
///
/// For `kind="cli"` rows, drops the cached entry, re-shells-out, and
/// stores the fresh result. Returns the fresh `Vec<ModelInfo>` in the
/// 200 body so the caller can update its UI without a second GET.
pub async fn refresh_provider_models(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<Vec<crate::agent::ModelInfo>>>, AppError> {
    let provider = ProviderStore::get_by_id(&state.db, &id)?;

    if provider.kind != "cli" {
        return Err(AppError::validation_with_code(
            "invalid_kind",
            format!(
                "POST /api/providers/:id/models only supports kind=cli; \
                 this provider is kind={}",
                provider.kind
            ),
        ));
    }

    // Drop the cached entry (no-op if absent).
    state.registry.invalidate_models(&id);

    // Re-shell-out fresh. The shell-out maps shell failures to
    // `ProviderError`, which the IntoResponse impl renders as 502
    // (`provider_error`).
    let models = list_cli_models_via_shell(&provider).await?;
    state.registry.put_cached_models(&id, models.clone());
    Ok(Json(DataResponse { data: models }))
}

/// Validate provider name: 1-100 chars after trimming.
fn validate_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::validation("name must not be empty"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(AppError::validation(format!(
            "name must be at most {} characters",
            MAX_NAME_LEN
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::Value;
    use std::path::Path;
    use tower::ServiceExt;

    /// Build a test app with provider routes.
    fn test_app() -> Router {
        test_app_with_state().0
    }

    /// Same as `test_app`, but returns the `(Router, AppState)` pair so
    /// tests that need to seed agents (and therefore warm the health
    /// cache) can keep a handle to the registry. The default `test_app`
    /// keeps the simpler 1-return shape for the existing CRUD tests.
    fn test_app_with_state() -> (Router, AppState) {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let active_child_processes =
            std::sync::Arc::new(crate::service::ActiveChildProcesses::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db,
            registry,
            active_sessions,
            active_child_processes,
            sse_manager,
            specialists,
            tools,
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());

        let router = Router::new()
            .route(
                "/api/providers",
                axum::routing::get(list_providers).post(create_provider),
            )
            .route(
                "/api/providers/{id}",
                axum::routing::delete(delete_provider),
            )
            .route(
                "/api/providers/{id}/models",
                axum::routing::get(list_provider_models).post(refresh_provider_models),
            )
            .layer(axum::Extension(state.clone()))
            .layer(axum::Extension(start_time));
        (router, state)
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    fn sample_body() -> &'static str {
        r#"{
            "kind": "http",
            "type": "anthropic",
            "name": "My Anthropic",
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-test-123",
            "default_model": "claude-sonnet-4-20250514"
        }"#
    }

    #[tokio::test]
    async fn test_provider_crud() {
        let app = test_app();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider = &json["data"];
        assert_eq!(provider["type"], "anthropic");
        assert_eq!(provider["name"], "My Anthropic");
        let provider_id = provider["id"].as_str().unwrap().to_string();

        // LIST
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], provider_id);

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/providers/{}", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify deleted — list should be empty
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 0);
    }

    #[tokio::test]
    async fn test_provider_api_key_stripped() {
        let app = test_app();

        // Create provider with api_key
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // List providers — api_key should NOT appear in response
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let response_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !response_str.contains("sk-test-123"),
            "api_key must not appear in response"
        );

        // Verify provider metadata is still present
        let items = json["data"].as_array().unwrap();
        assert_eq!(items[0]["type"], "anthropic");
        assert_eq!(items[0]["name"], "My Anthropic");
    }

    #[tokio::test]
    async fn test_create_validation() {
        let app = test_app();

        // Empty name
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"type":"anthropic","name":"","base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Name too long (101 chars)
        let long_name = "a".repeat(101);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"type":"anthropic","name":"{}","base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}}"#,
                        long_name
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Empty api_key
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"type":"anthropic","name":"Test","base_url":"https://api.anthropic.com","api_key":"","default_model":"claude-sonnet-4-20250514"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Empty base_url
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"type":"anthropic","name":"Test","base_url":"","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Empty default_model
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"type":"anthropic","name":"Test","base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":""}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Unsupported type
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"type":"openai","name":"Test","base_url":"https://api.openai.com","api_key":"sk-test","default_model":"gpt-4"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_not_found() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/providers/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_provider_delete_conflict() {
        let app_inner = test_app();

        // Create provider
        let response = app_inner
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let _provider_id = json["data"]["id"].as_str().unwrap().to_string();

        // Manually insert a session referencing this provider
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let active_child_processes =
            std::sync::Arc::new(crate::service::ActiveChildProcesses::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
            active_child_processes,
            sse_manager,
            specialists,
            tools,
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };

        // Insert provider into this DB
        let config_json = serde_json::json!({
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-test-123",
            "default_model": "claude-sonnet-4-20250514"
        })
        .to_string();
        ProviderStore::create(&db, "anthropic", "Test", &config_json).unwrap();

        // Get the provider we just created
        let providers = ProviderStore::list(&db).unwrap();
        let pid = &providers[0].id;

        // Insert workspace + session
        let ws_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'test', 'active', ?2, ?2)",
                rusqlite::params![ws_id, now],
            )
            .unwrap();

        let session_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'connecting', ?4, ?4)",
                rusqlite::params![session_id, ws_id, pid, now],
            )
            .unwrap();

        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());
        let app = Router::new()
            .route(
                "/api/providers/{id}",
                axum::routing::delete(delete_provider),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time));

        // DELETE should return 409
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/providers/{}", pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_list_models() {
        let app = test_app();

        // Create provider
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider_id = json["data"]["id"].as_str().unwrap().to_string();

        // GET models
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        // AnthropicAgent::list_models returns empty vec in v1
        let models = json["data"].as_array().unwrap();
        assert!(
            models.is_empty(),
            "list_models should return empty vec in v1"
        );
    }

    #[tokio::test]
    async fn test_list_models_not_found() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers/nonexistent/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -----------------------------------------------------------------
    // feat-039 named tests
    // -----------------------------------------------------------------

    fn cli_body() -> &'static str {
        r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "My Claude Code",
            "default_model": "claude-sonnet-4-5",
            "binary_path": "/usr/local/bin/claude",
            "args_json": "[\"--verbose\"]",
            "env_json": "{\"LOG_LEVEL\":\"info\"}",
            "permission_mode": "accept-edits"
        }"#
    }

    /// 1. HTTP CRUD round-trips the new `kind` field and the canonical
    /// `default_model` column.
    #[tokio::test]
    async fn test_provider_kind_http_crud() {
        let app = test_app();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider = &json["data"];
        assert_eq!(provider["kind"], "http");
        assert_eq!(provider["type"], "anthropic");
        assert_eq!(provider["default_model"], "claude-sonnet-4-20250514");
        assert!(provider["binary_path"].is_null());
        assert!(provider["args_json"].is_null());
        assert!(provider["env_json"].is_null());
        assert!(provider["permission_mode"].is_null());
        let provider_id = provider["id"].as_str().unwrap().to_string();

        // GET via LIST
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "http");

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/providers/{}", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// 2. CLI CRUD round-trips the CLI-only fields and confirms HTTP-only
    /// fields are absent.
    #[tokio::test]
    async fn test_provider_kind_cli_crud() {
        let app = test_app();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(cli_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider = &json["data"];
        assert_eq!(provider["kind"], "cli");
        assert_eq!(provider["default_model"], "claude-sonnet-4-5");
        assert_eq!(provider["binary_path"], "/usr/local/bin/claude");
        assert_eq!(provider["args_json"], "[\"--verbose\"]");
        assert_eq!(provider["env_json"], "{\"LOG_LEVEL\":\"info\"}");
        assert_eq!(provider["permission_mode"], "accept-edits");
        // API key is never present on the wire (struct field is HTTP-only).
        assert!(provider.get("api_key").is_none());
        // base_url is HTTP-only and was not set on this CLI row.
        assert!(provider["base_url"].is_null());
        let provider_id = provider["id"].as_str().unwrap().to_string();

        // DELETE (no sessions reference it, so 200)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/providers/{}", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// 3. Validation rejects malformed requests on both kinds.
    #[tokio::test]
    async fn test_provider_kind_validation() {
        let app = test_app();

        // kind=http with binary_path set → 400 invalid_field
        let body = r#"{
            "kind": "http",
            "type": "anthropic",
            "name": "Bad",
            "base_url": "https://x",
            "api_key": "sk-x",
            "default_model": "m",
            "binary_path": "/bin/x"
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_field");

        // kind=cli without binary_path → 400 missing_field
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Bad",
            "default_model": "m",
            "permission_mode": "default"
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "missing_field");

        // kind=cli with api_key set → 400 invalid_field
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Bad",
            "default_model": "m",
            "binary_path": "/bin/x",
            "permission_mode": "default",
            "api_key": "sk-x"
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_field");

        // kind=garbage → 400 invalid_kind
        let body = r#"{
            "kind": "garbage",
            "type": "anthropic",
            "name": "Bad",
            "default_model": "m",
            "binary_path": "/bin/x",
            "permission_mode": "default"
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_kind");

        // kind=cli with bad args_json → 400
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Bad",
            "default_model": "m",
            "binary_path": "/bin/x",
            "permission_mode": "default",
            "args_json": "not-json"
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        // args_json parse error uses the default `validation_error` code
        // (the handler calls `AppError::validation(format!(...))` without
        // an explicit code).
        assert_eq!(resp_json["error"]["code"], "validation_error");
        let msg = resp_json["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("args_json"),
            "error message should name the offending field: {msg}"
        );

        // kind=cli with relative binary_path → 400 invalid_field
        // (feat-042 hardening: the shell-out resolves against the
        // server's cwd, so relative paths are an ambiguity footgun).
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Bad",
            "default_model": "m",
            "binary_path": "claude",
            "permission_mode": "default"
        }"#;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_field");
        let msg = resp_json["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("absolute"),
            "error message should mention absolute path: {msg}"
        );
    }

    /// 4. api_key is stripped from responses for both kinds.
    #[tokio::test]
    async fn test_provider_api_key_stripped_across_kinds() {
        let app = test_app();

        // Create HTTP provider with api_key
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Create CLI provider (no api_key)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(cli_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // LIST
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let response_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !response_str.contains("sk-test-123"),
            "api_key must not appear in any response"
        );

        // Verify CLI row has no api_key field
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            assert!(item.get("api_key").is_none(), "api_key must be absent");
        }
    }

    /// 5. Migration 012 backfills `kind='http'` and `default_model`
    /// from `config_json` for pre-existing rows.
    ///
    /// Strategy: open a v12 DB, replace the `providers` table with its
    /// pre-012 5-column shape, insert a legacy row, then re-apply
    /// migration 012 (ALTER TABLE + UPDATE backfill).
    #[tokio::test]
    async fn test_provider_migration_backfills_http() {
        use crate::db::Db;
        use crate::store::providers::ProviderStore;

        let path = std::env::temp_dir().join("weave-test-migration-backfill.db");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        // 1. Open a fresh DB at v12.
        let db = Db::open(&path).expect("open temp db");

        // 2. Simulate a pre-feat-039 state: drop the migration-012
        //    columns (so the row shape is the 5 pre-012 columns), then
        //    INSERT a legacy row.
        db.conn()
            .execute_batch(
                "ALTER TABLE providers DROP COLUMN kind;
                 ALTER TABLE providers DROP COLUMN default_model;
                 ALTER TABLE providers DROP COLUMN binary_path;
                 ALTER TABLE providers DROP COLUMN args_json;
                 ALTER TABLE providers DROP COLUMN env_json;
                 ALTER TABLE providers DROP COLUMN permission_mode;",
            )
            .expect("drop migration-012 columns");
        // SQLite stores the dropped columns in the table; we need to
        // recreate the table for the legacy shape.
        db.conn()
            .execute_batch(
                "CREATE TABLE providers_legacy (
                    id TEXT PRIMARY KEY,
                    type TEXT NOT NULL,
                    name TEXT NOT NULL,
                    config_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO providers_legacy SELECT id, type, name, config_json, created_at FROM providers;
                 DROP TABLE providers;
                 ALTER TABLE providers_legacy RENAME TO providers;",
            )
            .expect("recreate legacy shape");

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let legacy_config = r#"{"base_url":"https://api.anthropic.com","api_key":"sk-test","default_model":"claude-sonnet-4-20250514"}"#;
        db.conn()
            .execute(
                "INSERT INTO providers (id, type, name, config_json, created_at)
                 VALUES (?1, 'anthropic', 'Legacy', ?2, ?3)",
                rusqlite::params![id, legacy_config, now],
            )
            .expect("insert legacy provider");

        // 3. Re-apply migration 012 (ALTER TABLE + backfill UPDATE).
        db.conn()
            .execute_batch(include_str!("../migrations/012_provider_runtime_kind.sql"))
            .expect("apply migration 012");

        // 4. Verify the row was backfilled.
        let providers = ProviderStore::list(&db).expect("list providers");
        assert_eq!(providers.len(), 1, "legacy row preserved");
        let p = &providers[0];
        assert_eq!(p.kind, "http", "kind backfilled to 'http'");
        assert_eq!(
            p.default_model.as_deref(),
            Some("claude-sonnet-4-20250514"),
            "default_model backfilled from config_json"
        );
        assert!(p.binary_path.is_none());
        assert!(p.args_json.is_none());
        assert!(p.env_json.is_none());
        assert!(p.permission_mode.is_none());

        // Cleanup
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// 6. CLI rows ARE dispatchable as of feat-042: the
    /// `GET /api/providers/:id/models` endpoint shells out to the
    /// registered binary and returns the parsed model list. Replaces
    /// the pre-feat-042 `test_provider_cli_row_not_yet_dispatchable`
    /// (which asserted a 501 and a literal "feat-042" string).
    #[tokio::test]
    async fn test_model_cache_miss_shells_out() {
        use std::os::unix::fs::PermissionsExt;

        // The CLI row's binary path is set via the create_provider
        // endpoint, which validates `binary_path` is non-empty but does
        // NOT check that the binary exists (that lands in feat-051's
        // dispatch adapter). The test therefore points the row at a
        // tempfile bash script that emits the canonical bare-array
        // shape.
        let tmp = tempfile::TempDir::new().unwrap();
        // Basename must be in feat-051's allowlist (`claude` or
        // `fake_cli`); use `fake_cli` so the create-CLI-row path
        // doesn't reject the row before the model-cache shell-out
        // gets a chance to run.
        let script_path = tmp.path().join("fake_cli");
        std::fs::write(
            &script_path,
            "#!/bin/sh\n\
             echo '[{\"id\":\"a\",\"name\":\"A\",\"context_window\":1000},{\"id\":\"b\",\"name\":\"B\",\"context_window\":2000}]'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cli_body_with_binary = format!(
            r#"{{
                "kind": "cli",
                "type": "anthropic",
                "name": "My Claude Code",
                "default_model": "claude-sonnet-4-5",
                "binary_path": "{}",
                "args_json": "[]",
                "env_json": "{{}}",
                "permission_mode": "accept-edits"
            }}"#,
            script_path.display()
        );

        let app = test_app();

        // Create the CLI row.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(cli_body_with_binary))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider_id = json["data"]["id"].as_str().unwrap().to_string();

        // GET models — first call is a cache miss, must shell out.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let models = json["data"].as_array().unwrap();
        assert_eq!(
            models.len(),
            2,
            "shell-out must yield the two model entries"
        );
        assert_eq!(models[0]["id"], "a");
        assert_eq!(models[1]["context_window"], 2000);
    }

    /// 4. (feat-042 verification) `POST /api/providers/:id/models`
    /// force-refreshes the cache for a CLI row and returns the fresh
    /// model list. After a refresh that re-shells-out with a different
    /// script, the second GET returns the new models (cache was
    /// updated by the refresh, not just invalidated).
    #[tokio::test]
    async fn test_model_cache_refresh_endpoint() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        // Same allowlist constraint as the miss test above — basename
        // `fake_cli` is in feat-051's dispatcher allowlist.
        let script_path = tmp.path().join("fake_cli");
        std::fs::write(
            &script_path,
            "#!/bin/sh\n\
             echo '[{\"id\":\"v1\",\"name\":\"V1\",\"context_window\":1000}]'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cli_body_with_binary = format!(
            r#"{{
                "kind": "cli",
                "type": "anthropic",
                "name": "Refresher",
                "default_model": "m",
                "binary_path": "{}",
                "args_json": "[]",
                "env_json": "{{}}",
                "permission_mode": "default"
            }}"#,
            script_path.display()
        );

        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(cli_body_with_binary))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let provider_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Prime the cache with v1.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["data"][0]["id"], "v1");

        // Rewrite the script to emit v2.
        std::fs::write(
            &script_path,
            "#!/bin/sh\n\
             echo '[{\"id\":\"v2\",\"name\":\"V2\",\"context_window\":2000}]'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        // POST refresh — drops the cached entry, re-shells-out, stores v2.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(
            json["data"][0]["id"], "v2",
            "refresh body must return the fresh model list"
        );

        // Subsequent GET returns v2 (cache was updated, not just dropped).
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(
            json["data"][0]["id"], "v2",
            "GET after refresh must return the refreshed model list"
        );
    }

    /// 5. (feat-042 verification) `POST /api/providers/:id/models`
    /// rejects `kind="http"` rows with a 400 carrying the
    /// `invalid_kind` code. The endpoint is CLI-only by design — the
    /// HTTP runtime has no shell-out to refresh.
    #[tokio::test]
    async fn test_model_cache_refresh_rejected_for_http() {
        let app = test_app();

        // Create an HTTP provider.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let provider_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // POST /models is rejected for HTTP.
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["error"]["code"], "invalid_kind");
    }

    /// 7. DELETE on a CLI provider that has referencing sessions
    /// returns 409 (the existing `has_sessions` check is kind-agnostic).
    #[tokio::test]
    async fn test_provider_remove_referenced() {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let active_child_processes =
            std::sync::Arc::new(crate::service::ActiveChildProcesses::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
            active_child_processes,
            sse_manager,
            specialists,
            tools,
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());
        let app = Router::new()
            .route(
                "/api/providers",
                axum::routing::get(list_providers).post(create_provider),
            )
            .route(
                "/api/providers/{id}",
                axum::routing::delete(delete_provider),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time));

        // Create CLI provider
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(cli_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let provider_id = json["data"]["id"].as_str().unwrap().to_string();

        // Insert a session referencing this provider
        let ws_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'test', 'active', ?2, ?2)",
                rusqlite::params![ws_id, now],
            )
            .unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'connecting', ?4, ?4)",
                rusqlite::params![session_id, ws_id, provider_id, now],
            )
            .unwrap();

        // DELETE returns 409
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/providers/{}", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    // -----------------------------------------------------------------
    // feat-051: env denylist + binary basename allowlist
    //
    // These tests lock in the request-time gates added in response to
    // the code-review feedback on the env-injection / arbitrary-binary
    // threat models. The gates reject with 400 *before* the row is
    // persisted, so a bad row never reaches the `list_cli_models_via_shell`
    // shell-out path.
    // -----------------------------------------------------------------

    #[test]
    fn test_validate_env_keys_rejects_dynamic_linker_keys() {
        use std::collections::BTreeMap;

        // Each of these keys is a real dynamic-linker / shell-loader
        // variable; the request is rejected with `invalid_field`.
        for dangerous in [
            "LD_PRELOAD",
            "ld_preload", // case-insensitive
            "LD_LIBRARY_PATH",
            "LD_AUDIT",
            "DYLD_INSERT_LIBRARIES",
            "PATH",
        ] {
            let mut env = BTreeMap::new();
            env.insert(dangerous.to_string(), "/tmp/evil".to_string());
            let err = validate_env_keys(&env).expect_err("must reject");
            match err {
                AppError::Validation { code, message } => {
                    assert_eq!(
                        code, "invalid_field",
                        "validate_env_keys({env:?}) should be invalid_field"
                    );
                    assert!(
                        message.contains(dangerous),
                        "error message must name the offending key, got: {message}"
                    );
                }
                other => panic!("expected AppError::Validation, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_validate_env_keys_allows_safe_keys() {
        use std::collections::BTreeMap;

        // None of these can hijack the dynamic linker or the loader.
        let mut env = BTreeMap::new();
        env.insert("LOG_LEVEL".to_string(), "info".to_string());
        env.insert("WEAVE_PROJECT".to_string(), "demo".to_string());
        env.insert("HOME".to_string(), "/home/agent".to_string());
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-...".to_string());
        assert!(validate_env_keys(&env).is_ok(), "safe keys must pass");
    }

    #[tokio::test]
    async fn test_create_cli_provider_rejects_dangerous_env() {
        let app = test_app();
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Evil",
            "default_model": "m",
            "binary_path": "/usr/local/bin/claude",
            "env_json": "{\"LD_PRELOAD\":\"/tmp/evil.so\"}",
            "permission_mode": "default"
        }"#;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "LD_PRELOAD must be rejected at request time"
        );
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_field");
        assert!(
            resp_json["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("LD_PRELOAD"),
            "error must name the offending key, got: {resp_json}"
        );
    }

    #[tokio::test]
    async fn test_create_cli_provider_rejects_unknown_basename() {
        let app = test_app();
        // `/bin/sh` is the canonical arbitrary-binary attack surface
        // the basename allowlist exists to block.
        let body = r#"{
            "kind": "cli",
            "type": "anthropic",
            "name": "Sh",
            "default_model": "m",
            "binary_path": "/bin/sh",
            "permission_mode": "default"
        }"#;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "unrecognized basename must be rejected at request time, not just warned"
        );
        let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp_json = extract_json(&resp_body);
        assert_eq!(resp_json["error"]["code"], "invalid_field");
        assert!(
            resp_json["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("sh"),
            "error must name the basename, got: {resp_json}"
        );
    }

    // ---- feat-053: healthy field on GET /api/providers ----

    /// A freshly booted server with no prior `cached_health_summary`
    /// call must mark every provider `healthy: false`. The wizard's
    /// Step 1 uses this to hide unproven providers rather than
    /// offering broken ones (cold-start safety).
    #[tokio::test]
    async fn test_provider_healthy_field_default_false_when_cache_cold() {
        let (app, _state) = test_app_with_state();

        // Create one HTTP provider so the list isn't empty.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // GET /api/providers — cache is cold (no probe has run).
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["healthy"], false,
            "cold cache must surface healthy: false"
        );
    }

    /// After a `cached_health_summary` call warms the cache with a
    /// healthy agent, GET /api/providers must reflect that. This is
    /// the hot path: a periodic health-probe (or a call from the
    /// aggregate `/api/health`) populates the snapshot, and the
    /// wizard sees live status within the 10s TTL.
    #[tokio::test]
    async fn test_provider_healthy_field_reflects_cache_after_probe() {
        use crate::agent::registry::ProviderRegistry;
        use crate::agent::CodingAgent;
        use crate::agent::ProviderHealth;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (app, state) = test_app_with_state();

        // Create a provider row in the DB so the list isn't empty.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_body()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let provider_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Register a stub agent that reports healthy=true; warm the
        // cache via the aggregate accessor. The 10s TTL window is
        // plenty for the immediate GET.
        struct StubHealthy;
        #[async_trait::async_trait]
        impl CodingAgent for StubHealthy {
            fn provider_type(&self) -> &str {
                "stub"
            }
            fn display_name(&self) -> &str {
                "stub"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _req: crate::agent::MessageRequest,
                _turn: &crate::agent::turn_context::TurnContext,
            ) -> Result<
                std::pin::Pin<
                    Box<
                        dyn futures_util::Stream<
                                Item = Result<
                                    crate::agent::StreamEvent,
                                    crate::error::ProviderError,
                                >,
                            > + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                Err(crate::error::ProviderError::Unreachable("stub".into()))
            }
            async fn health_check(&self) -> Result<ProviderHealth, crate::error::ProviderError> {
                Ok(ProviderHealth {
                    healthy: true,
                    latency_ms: 1,
                    error: None,
                })
            }
        }
        state
            .registry
            .add_agent(&provider_id, std::sync::Arc::new(StubHealthy));
        let _ = state.registry.cached_health_summary().await;

        // GET /api/providers — cache is now warm with healthy=true
        // for our stub.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["healthy"], true,
            "warmed cache with healthy=true must surface healthy: true"
        );

        // Suppress the unused-imports lint for the types named
        // above for documentation symmetry with the other registry
        // test (`test_cached_health_summary_cache_hit_within_ttl`).
        let _ = (
            AtomicUsize::new(0).load(Ordering::SeqCst),
            ProviderRegistry::new,
        );
    }
}
