use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::sessions::{MessagePage, MessageStore, SessionPage, SessionStore};
use crate::AppState;

const DEFAULT_PAGE_LIMIT: u32 = 100;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl ListParams {
    pub fn effective_limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, 100)
    }
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub provider_id: String,
    pub specialist_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub parent_session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/sessions
pub async fn create_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<
    (
        StatusCode,
        Json<DataResponse<crate::store::sessions::Session>>,
    ),
    AppError,
> {
    // Validate workspace exists
    crate::store::workspaces::WorkspaceStore::get_by_id(&state.db, &workspace_id)?;

    let session = SessionStore::create(
        &state.db,
        &workspace_id,
        &body.provider_id,
        body.specialist_id.as_deref(),
        body.model.as_deref(),
        body.cwd.as_deref(),
        body.parent_session_id.as_deref(),
    )?;

    Ok((StatusCode::CREATED, Json(DataResponse { data: session })))
}

/// GET /api/workspaces/{wid}/sessions
pub async fn list_sessions(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataResponse<SessionPage>>, AppError> {
    let page = SessionStore::list_by_workspace(
        &state.db,
        &workspace_id,
        params.cursor.as_deref(),
        params.effective_limit(),
    )?;
    Ok(Json(DataResponse { data: page }))
}

/// GET /api/sessions/{id}
pub async fn get_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<crate::store::sessions::Session>>, AppError> {
    let session = SessionStore::get_by_id(&state.db, &id)?;
    Ok(Json(DataResponse { data: session }))
}

/// DELETE /api/sessions/{id}
pub async fn delete_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    SessionStore::delete(&state.db, &id)?;
    Ok(Json(DataResponse { data: () }))
}

/// PATCH /api/sessions/{id}/status
pub async fn update_session_status(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateStatusRequest>,
) -> Result<Json<DataResponse<crate::store::sessions::Session>>, AppError> {
    let session = SessionStore::update_status(&state.db, &id, &body.status)?;
    Ok(Json(DataResponse { data: session }))
}

/// GET /api/sessions/{sid}/history
pub async fn get_session_history(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataResponse<MessagePage>>, AppError> {
    let page = MessageStore::list_by_session(
        &state.db,
        &session_id,
        params.cursor.as_deref(),
        params.effective_limit(),
    )?;
    Ok(Json(DataResponse { data: page }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    fn test_app() -> (Router, String, String) {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
        };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());

        let (ws_id, provider_id) = crate::store::sessions::tests::seed_deps(&db);

        let router = Router::new()
            .route(
                "/api/workspaces/{wid}/sessions",
                axum::routing::get(list_sessions).post(create_session),
            )
            .route(
                "/api/sessions/{id}",
                axum::routing::get(get_session)
                    .patch(update_session_status)
                    .delete(delete_session),
            )
            .route(
                "/api/sessions/{sid}/history",
                axum::routing::get(get_session_history),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time));

        (router, ws_id, provider_id)
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn test_session_lifecycle() {
        let (app, ws_id, provider_id) = test_app();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}","model":"claude-sonnet-4-20250514","cwd":"/tmp"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let session = &json["data"];
        assert_eq!(session["workspace_id"], ws_id);
        assert_eq!(session["provider_id"], provider_id);
        assert_eq!(session["status"], "connecting");
        assert_eq!(session["model"], "claude-sonnet-4-20250514");
        assert_eq!(session["cwd"], "/tmp");
        let session_id = session["id"].as_str().unwrap().to_string();

        // GET
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}", session_id))
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
        assert_eq!(json["data"]["id"], session_id);

        // UPDATE STATUS
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["data"]["status"], "ready");

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify deleted
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_state_transitions() {
        let (app, ws_id, provider_id) = test_app();

        // Create session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // connecting -> ready (valid)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // ready -> completed (valid)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"completed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // completed -> ready (invalid — terminal)
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_session_not_found() {
        let (app, _, _) = test_app();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_invalid_provider() {
        let (app, ws_id, _) = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"provider_id":"nonexistent"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_session_list_empty() {
        let (app, ws_id, _) = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
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
        let items = json["data"]["data"].as_array().unwrap();
        assert!(items.is_empty());
    }
}
