use axum::Json;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::specialist::Specialist;
use crate::AppState;

/// GET /api/specialists
///
/// Returns all loaded specialists. The `system_prompt` field is excluded
/// from the response via `#[serde(skip)]` on the `Specialist` struct.
pub async fn list_specialists(
    axum::Extension(state): axum::Extension<AppState>,
) -> Result<Json<DataResponse<Vec<Specialist>>>, AppError> {
    let specialists: Vec<Specialist> = state.specialists.all().into_iter().cloned().collect();
    Ok(Json(DataResponse { data: specialists }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use axum::routing::get;
    use axum::Router;
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::db::Db;
    use crate::specialist::{Specialist, SpecialistRegistry};
    use crate::AppState;

    fn make_state(specialists: SpecialistRegistry) -> AppState {
        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        AppState {
            db,
            registry: Arc::new(crate::agent::registry::ProviderRegistry::new()),
            active_sessions: Arc::new(crate::service::ActiveSessions::new()),
            sse_manager: Arc::new(crate::sse::SseManager::new()),
            specialists: Arc::new(specialists),
            tools: Arc::new(crate::tools::ToolRegistry::new()),
        }
    }

    fn test_app() -> Router {
        let mut specialists = SpecialistRegistry::new();
        specialists.insert(Specialist {
            name: "test-specialist".to_string(),
            description: "A test specialist".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            tool_profile: Some("implementation".to_string()),
            tags: vec!["test".to_string()],
            system_prompt: "SECRET PROMPT".to_string(),
        });

        Router::new()
            .route("/api/specialists", get(super::list_specialists))
            .layer(axum::Extension(make_state(specialists)))
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn test_list_specialists() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .uri("/api/specialists")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["name"], "test-specialist");
        assert_eq!(data[0]["description"], "A test specialist");
        assert_eq!(data[0]["model"], "claude-sonnet-4-20250514");
        assert_eq!(data[0]["tool_profile"], "implementation");
        assert_eq!(data[0]["tags"], serde_json::json!(["test"]));
    }

    #[tokio::test]
    async fn test_list_specialists_excludes_system_prompt() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .uri("/api/specialists")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let data = json["data"].as_array().unwrap();

        // system_prompt must not appear in the response
        assert!(data[0].get("system_prompt").is_none());
    }

    #[tokio::test]
    async fn test_list_specialists_empty() {
        let app = Router::new()
            .route("/api/specialists", get(super::list_specialists))
            .layer(axum::Extension(make_state(SpecialistRegistry::new())));

        let req = axum::http::Request::builder()
            .uri("/api/specialists")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 0);
    }
}
