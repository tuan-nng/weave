use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::workspaces::{WorkspacePage, WorkspaceStore};
use crate::AppState;

const DEFAULT_NAME: &str = "default";
const MAX_NAME_LEN: usize = 100;
const DEFAULT_PAGE_LIMIT: u32 = 100;

#[derive(Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct PatchRequest {
    pub name: String,
}

/// GET /api/workspaces
pub async fn list_workspaces(
    axum::Extension(state): axum::Extension<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataResponse<WorkspacePage>>, AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, 100);

    let page = WorkspaceStore::list(&state.db, params.cursor.as_deref(), limit)?;
    Ok(Json(DataResponse { data: page }))
}

/// POST /api/workspaces
pub async fn create_workspace(
    axum::Extension(state): axum::Extension<AppState>,
    Json(body): Json<CreateRequest>,
) -> Result<
    (
        StatusCode,
        Json<DataResponse<crate::store::workspaces::Workspace>>,
    ),
    AppError,
> {
    let name = body.name.trim();
    validate_name(name)?;

    let workspace = WorkspaceStore::create(&state.db, name)?;
    Ok((StatusCode::CREATED, Json(DataResponse { data: workspace })))
}

/// GET /api/workspaces/{id}
pub async fn get_workspace(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<crate::store::workspaces::Workspace>>, AppError> {
    let workspace = WorkspaceStore::get_by_id(&state.db, &id)?;
    Ok(Json(DataResponse { data: workspace }))
}

/// PATCH /api/workspaces/{id}
pub async fn update_workspace(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchRequest>,
) -> Result<Json<DataResponse<crate::store::workspaces::Workspace>>, AppError> {
    let name = body.name.trim();
    validate_name(name)?;

    // Fetch current workspace to check default protection
    let current = WorkspaceStore::get_by_id(&state.db, &id)?;
    if current.name == DEFAULT_NAME {
        return Err(AppError::Validation(
            "cannot rename default workspace".into(),
        ));
    }

    let workspace = WorkspaceStore::update_name(&state.db, &id, name)?;
    Ok(Json(DataResponse { data: workspace }))
}

/// DELETE /api/workspaces/{id}
pub async fn delete_workspace(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    // Fetch current workspace to check default protection
    let current = WorkspaceStore::get_by_id(&state.db, &id)?;
    if current.name == DEFAULT_NAME {
        return Err(AppError::Validation(
            "cannot delete default workspace".into(),
        ));
    }

    WorkspaceStore::delete(&state.db, &id, &current.name)?;
    Ok(Json(DataResponse { data: () }))
}

/// Validate workspace name: 1-100 chars after trimming.
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

    /// Build a test app with workspace routes and a seeded default workspace.
    fn test_app() -> Router {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let state = AppState { db, registry };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());

        Router::new()
            .route(
                "/api/workspaces",
                axum::routing::get(list_workspaces).post(create_workspace),
            )
            .route(
                "/api/workspaces/{id}",
                axum::routing::get(get_workspace)
                    .patch(update_workspace)
                    .delete(delete_workspace),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time))
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn test_workspace_crud() {
        let app = test_app();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"my-project"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let workspace = &json["data"];
        assert_eq!(workspace["name"], "my-project");
        assert_eq!(workspace["status"], "active");
        let ws_id = workspace["id"].as_str().unwrap().to_string();

        // GET
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}", ws_id))
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
        assert_eq!(json["data"]["name"], "my-project");

        // LIST (should include default + my-project)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
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
        assert_eq!(items.len(), 2, "should have default + my-project");

        // PATCH
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/workspaces/{}", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed-project"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["data"]["name"], "renamed-project");

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{}", ws_id))
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
                    .uri(format!("/api/workspaces/{}", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_workspace_default_seed() {
        let app = test_app();

        // List should contain the default workspace
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"]["data"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "default");

        let default_id = items[0]["id"].as_str().unwrap().to_string();

        // Cannot rename default workspace
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/workspaces/{}", default_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Cannot delete default workspace
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{}", default_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_name_validation() {
        let app = test_app();

        // Empty name
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Whitespace-only name
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Name too long (101 chars)
        let long_name = "a".repeat(101);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"name":"{}"}}"#, long_name)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_unique_name() {
        let app = test_app();

        // Create first
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"duplicate"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Create duplicate
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"duplicate"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
