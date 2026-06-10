use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::collections::BTreeMap;

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
pub async fn list_providers(
    axum::Extension(state): axum::Extension<AppState>,
) -> Result<Json<DataResponse<Vec<Provider>>>, AppError> {
    let providers = ProviderStore::list(&state.db)?;
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
    if binary_path.trim().is_empty() {
        return Err(AppError::validation("binary_path must not be empty"));
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

    let provider = ProviderStore::create_cli(
        &state.db,
        &body.provider_type,
        name,
        default_model.trim(),
        binary_path.trim(),
        &args_json_canonical,
        &env_json_canonical,
        permission_mode.trim(),
    )?;

    Ok((StatusCode::CREATED, Json(DataResponse { data: provider })))
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
/// `kind="cli"` rows return 501 `NotImplemented` — the CLI model list
/// lands in feat-042 (per-adapter model cache) and is intentionally not
/// in feat-039 scope.
pub async fn list_provider_models(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<Vec<crate::agent::ModelInfo>>>, AppError> {
    // Short-circuit on kind BEFORE looking up the agent so CLI rows
    // produce a clear 501 rather than a spurious 404 from the registry.
    let provider = ProviderStore::get_by_id(&state.db, &id)?;
    if provider.kind == "cli" {
        return Err(AppError::NotImplemented(
            "CLI model list not available until feat-042".to_string(),
        ));
    }

    let agent = state.registry.get_agent(&id)?;
    let models = agent.list_models().await?;
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
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db,
            registry,
            active_sessions,
            sse_manager,
            specialists,
            tools,
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());

        Router::new()
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
                axum::routing::get(list_provider_models),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time))
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
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
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

    /// 6. CLI rows can be created but are not yet dispatchable; the
    /// `GET /api/providers/:id/models` endpoint returns 501.
    #[tokio::test]
    async fn test_provider_cli_row_not_yet_dispatchable() {
        let app = test_app();

        // Create CLI row
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

        // GET models on a CLI row returns 501
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/providers/{}/models", provider_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["error"]["code"], "not_implemented");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("feat-042"));
    }

    /// 7. DELETE on a CLI provider that has referencing sessions
    /// returns 409 (the existing `has_sessions` check is kind-agnostic).
    #[tokio::test]
    async fn test_provider_remove_referenced() {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
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
}
