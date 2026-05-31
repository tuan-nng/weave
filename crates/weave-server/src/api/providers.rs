use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::providers::{Provider, ProviderStore};
use crate::AppState;

const MAX_NAME_LEN: usize = 100;

#[derive(Deserialize)]
pub struct CreateProviderRequest {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
}

/// GET /api/providers
pub async fn list_providers(
    axum::Extension(state): axum::Extension<AppState>,
) -> Result<Json<DataResponse<Vec<Provider>>>, AppError> {
    let providers = ProviderStore::list(&state.db)?;
    Ok(Json(DataResponse { data: providers }))
}

/// POST /api/providers
pub async fn create_provider(
    axum::Extension(state): axum::Extension<AppState>,
    Json(body): Json<CreateProviderRequest>,
) -> Result<(StatusCode, Json<DataResponse<Provider>>), AppError> {
    // Validate name
    let name = body.name.trim();
    validate_name(name)?;

    // Validate provider type
    if body.provider_type != "anthropic" {
        return Err(AppError::Validation(format!(
            "unsupported provider type: '{}' (only 'anthropic' supported in v1)",
            body.provider_type
        )));
    }

    // Validate config fields
    if body.base_url.trim().is_empty() {
        return Err(AppError::Validation("base_url must not be empty".into()));
    }
    if body.api_key.trim().is_empty() {
        return Err(AppError::Validation("api_key must not be empty".into()));
    }
    if body.default_model.trim().is_empty() {
        return Err(AppError::Validation(
            "default_model must not be empty".into(),
        ));
    }

    // Build config_json
    let config_json = serde_json::json!({
        "base_url": body.base_url.trim(),
        "api_key": body.api_key.trim(),
        "default_model": body.default_model.trim(),
    })
    .to_string();

    // Create agent to validate config
    let agent =
        crate::agent::registry::ProviderRegistry::create_agent(&body.provider_type, &config_json)
            .map_err(|e| AppError::Validation(format!("invalid provider config: {e}")))?;

    // Persist to DB
    let provider = ProviderStore::create(&state.db, &body.provider_type, name, &config_json)?;

    // Register agent in runtime registry
    state.registry.add_agent(&provider.id, agent);

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
pub async fn list_provider_models(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<Vec<crate::agent::ModelInfo>>>, AppError> {
    let agent = state.registry.get_agent(&id)?;
    let models = agent.list_models().await?;
    Ok(Json(DataResponse { data: models }))
}

/// Validate provider name: 1-100 chars after trimming.
fn validate_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::Validation("name must not be empty".into()));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(AppError::Validation(format!(
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
        let state = AppState {
            db,
            registry,
            active_sessions,
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
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
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
}
