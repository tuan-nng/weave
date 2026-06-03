//! HTTP API for codebases (feat-032).
//!
//! Route layout (registered in `api/mod.rs`):
//! - `POST   /api/workspaces/{wid}/codebases`  create_codebase
//! - `GET    /api/workspaces/{wid}/codebases`  list_codebases
//! - `GET    /api/codebases/{id}`              get_codebase (composite w/ git)
//! - `DELETE /api/codebases/{id}`              delete_codebase
//!
//! `get_codebase` is intentionally at `/api/codebases/:id` (not
//! `/api/workspaces/:wid/codebases/:id`): the cross-workspace 404
//! guard runs inside the handler by comparing the row's
//! `workspace_id` to the caller-supplied `wid` query param, so the
//! URL shape stays symmetric with the `boards` resources that take
//! `wid` in the path. The frontend uses a workspace-scoped URL
//! (`/workspaces/:wid/codebases/:cid`) that submits `?wid=...` on
//! detail fetches — see `web/src/lib/api.ts::codebases.get`.
//!
//! Composite GET shape (`CodebaseDetail`): the row is always
//! returned; `git_status` is populated when the path is a git repo
//! AND git is callable; `git_error` is populated with a one-line
//! explanation when git fails. A 404 from `git rev-parse` is the
//! common case (registered a non-git directory by mistake) — we
//! surface it as `git_error: Some("Path is not a git repository")`
//! rather than a 5xx, so the client can show the user a fix hint.

use std::path::PathBuf;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::codebases::{
    build_detail, Codebase, CodebaseDetail, CodebaseStore, GitCommit, GitStatus,
};
use crate::tools::git::{log::parse_log, status::parse_status};
use crate::AppState;

// ---------------------------------------------------------------------------
// Request DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateCodebaseRequest {
    /// Absolute filesystem path. Required.
    pub path: String,
    /// Optional branch hint (display-only — the codebase tracks the
    /// working tree, not a ref).
    pub branch: Option<String>,
    /// Optional human label (e.g. "Backend", "Mobile").
    pub label: Option<String>,
}

#[derive(Deserialize)]
pub struct GetCodebaseQuery {
    /// Workspace id; required to enforce the cross-workspace 404 guard.
    /// Comes in via `?wid=...`.
    pub wid: String,
}

const RECENT_COMMITS_LIMIT: usize = 5;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/codebases
pub async fn create_codebase(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateCodebaseRequest>,
) -> Result<(StatusCode, Json<DataResponse<Codebase>>), AppError> {
    let path = body.path.trim();
    validate_absolute_path(path)?;
    let path_exists = std::path::Path::new(path).is_dir();
    if !path_exists {
        return Err(AppError::Validation(format!(
            "codebase path '{}' does not exist or is not a directory",
            path
        )));
    }
    // Guard: must be a git repository. A registered codebase that
    // isn't a git repo would always error in the composite GET —
    // catching it at create time gives a clearer message.
    let pb = PathBuf::from(path);
    if !is_git_repo(&pb).await {
        return Err(AppError::Validation(format!(
            "codebase path '{}' is not a git repository",
            path
        )));
    }
    let codebase = CodebaseStore::create(
        &state.db,
        &workspace_id,
        path,
        body.branch.as_deref(),
        body.label.as_deref(),
    )?;
    Ok((StatusCode::CREATED, Json(DataResponse { data: codebase })))
}

/// GET /api/workspaces/{wid}/codebases
pub async fn list_codebases(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<DataResponse<Vec<Codebase>>>, AppError> {
    let codebases = CodebaseStore::list_by_workspace(&state.db, &workspace_id)?;
    Ok(Json(DataResponse { data: codebases }))
}

/// GET /api/codebases/{id}?wid={wid}
///
/// Composite: row + git status. The `wid` query param is required
/// for the cross-workspace 404 guard (matches the `boards` pattern at
/// `api/kanban.rs:194-201`).
pub async fn get_codebase(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GetCodebaseQuery>,
) -> Result<Json<DataResponse<CodebaseDetail>>, AppError> {
    let codebase = CodebaseStore::get_in_workspace(&state.db, &id, &q.wid)?;
    let git = compose_git_status(&codebase.path).await;
    let detail = build_detail(codebase, git);
    Ok(Json(DataResponse { data: detail }))
}

/// DELETE /api/codebases/{id}?wid={wid}
pub async fn delete_codebase(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GetCodebaseQuery>,
) -> Result<Json<DataResponse<()>>, AppError> {
    // Verify the codebase belongs to the requesting workspace before
    // mutating. Returns the same NotFound shape as a missing codebase
    // so an agent cannot delete codebases across workspaces.
    let codebase = CodebaseStore::get_in_workspace(&state.db, &id, &q.wid)?;
    CodebaseStore::delete(&state.db, &codebase.id)?;
    Ok(Json(DataResponse { data: () }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reject relative paths at the API boundary. The store stores
/// paths as-given; the handler is the validation layer.
fn validate_absolute_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(AppError::Validation(
            "codebase path must not be empty".into(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(AppError::Validation(format!(
            "codebase path '{}' must be absolute",
            path
        )));
    }
    Ok(())
}

/// `git rev-parse --git-dir` returns 0 inside a repo, non-zero
/// outside. Used at create time to reject non-git paths.
async fn is_git_repo(path: &PathBuf) -> bool {
    run_git_in_path(&["rev-parse", "--git-dir"], path)
        .await
        .is_ok()
}

/// Run a git command in `path`. Returns `Err(msg)` on spawn failure or
/// non-zero exit; `Ok(stdout)` otherwise. Async via
/// `tokio::process::Command`. Stays in the API layer (not the store)
/// because the store is for persistence, not subprocess orchestration.
async fn run_git_in_path(args: &[&str], path: &PathBuf) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => return Err(format!("failed to spawn git: {e}")),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Compose a `GitStatus` for the given path. Returns:
///   - `Ok(Some(status))` — happy path
///   - `Ok(None)` — path is not a git repo (rev-parse failed cleanly)
///   - `Err(msg)` — git was callable but reported a different error
///
/// Used by the composite GET handler. Implementation reuses the
/// private parsers in `tools::git` (now `pub(crate)`) so the
/// porcelain format is parsed identically to the agent-facing
/// `git_status` tool.
pub async fn compose_git_status(path: &str) -> Result<Option<GitStatus>, String> {
    let pb = PathBuf::from(path);
    if !pb.is_dir() {
        return Err(format!(
            "path '{}' does not exist or is not a directory",
            path
        ));
    }

    // Branch + dirty files. The two commands are independent — if
    // `log` fails (e.g. empty repo, no commits), we still return the
    // status branch and an empty recent_commits list.
    let status_output = match run_git_in_path(&["status", "--porcelain=v1", "-b"], &pb).await {
        Ok(s) => s,
        Err(e) => {
            // `git rev-parse --git-dir` would tell us if it's not a
            // repo. Run it as a probe to distinguish "not a repo"
            // (returns Ok(None)) from "git is broken" (Err).
            return match run_git_in_path(&["rev-parse", "--git-dir"], &pb).await {
                Ok(_) => Err(e),
                Err(repo_err) => Err(repo_err),
            };
        }
    };

    let (branch, staged, unstaged, untracked) = parse_status(&status_output);
    // Dirty = anything that isn't clean. The spec asks for the
    // union; the `git_status` tool returns the three lists
    // separately, but the composite endpoint flattens.
    let mut dirty_files: Vec<String> =
        Vec::with_capacity(staged.len() + unstaged.len() + untracked.len());
    dirty_files.extend(staged);
    dirty_files.extend(unstaged);
    dirty_files.extend(untracked);

    // Recent commits. `git log` on an empty repo exits non-zero
    // ("bad default revision") — treat as empty list, not an error.
    let log_output = run_git_in_path(
        &[
            "log",
            "--oneline",
            &format!("-n{RECENT_COMMITS_LIMIT}"),
            "--no-color",
        ],
        &pb,
    )
    .await
    .unwrap_or_default();
    let recent_commits: Vec<GitCommit> = if log_output.is_empty() {
        Vec::new()
    } else {
        parse_log(&log_output)
            .into_iter()
            .filter_map(|v: Value| {
                let hash = v.get("hash")?.as_str()?.to_string();
                let message = v.get("message")?.as_str()?.to_string();
                Some(GitCommit { hash, message })
            })
            .collect()
    };

    // If branch is empty AND there are no recent commits, the path
    // Note: an empty repo (no commits, no changes) produces an empty
    // porcelain output AND an empty `git log` output. We return
    // Ok(Some(empty)) for that case so the client renders an empty
    // status block instead of a "not a git repository" error. A
    // non-repo path fails the `status` call above and is mapped
    // separately to Ok(None) via `build_detail`.

    Ok(Some(GitStatus {
        branch,
        dirty_files,
        recent_commits,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_state;
    use crate::tools::git::git_test_support::git_init;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn test_app(state: AppState) -> Router {
        Router::new()
            .route(
                "/api/workspaces/{wid}/codebases",
                axum::routing::get(list_codebases).post(create_codebase),
            )
            .route(
                "/api/codebases/{id}",
                axum::routing::get(get_codebase).delete(delete_codebase),
            )
            .layer(axum::Extension(state))
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    fn default_workspace(state: &AppState) -> String {
        state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn test_codebase_crud() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);

        // Set up a real git repo on disk so the create-time guard passes.
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        let path = tmp.path().to_str().unwrap();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"path":"{}","label":"Test repo"}}"#,
                        path
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let codebase = extract_json(&body)["data"].clone();
        let cid = codebase["id"].as_str().unwrap().to_string();
        assert_eq!(codebase["path"], path);
        assert_eq!(codebase["label"], "Test repo");

        // LIST
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list = extract_json(&body)["data"].as_array().unwrap().clone();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], cid);

        // GET composite (with git status)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/codebases/{}?wid={}", cid, ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body)["data"].clone();
        assert_eq!(detail["codebase"]["id"], cid);
        // git_status is Some for a real git repo
        assert!(
            detail["git_status"].is_object(),
            "git_status should be populated for a real repo: {}",
            serde_json::to_string(&detail).unwrap()
        );
        assert_eq!(detail["git_status"]["branch"], "main");
        assert_eq!(detail["git_error"], Value::Null);

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/codebases/{}?wid={}", cid, ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // GET after delete → 404
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/codebases/{}?wid={}", cid, ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_rejects_relative_path() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"relative/path"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_rejects_nonexistent_path() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"path":"/this/path/does/not/exist/anywhere-xyz123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_rejects_non_git_directory() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_str().unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"path":"{}"}}"#, path)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_duplicate_path_in_workspace_returns_409() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        let path = tmp.path().to_str().unwrap();
        let body_str = format!(r#"{{"path":"{}"}}"#, path);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(body_str.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(body_str))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_cross_workspace_get_returns_404() {
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        let path = tmp.path().to_str().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"path":"{}"}}"#, path)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let cid = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET with the wrong workspace id
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/codebases/{}?wid=wrong-ws", cid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_codebase_git_status() {
        // Specifically asserts the `git_status` shape: branch,
        // dirty_files, recent_commits.
        let state = make_test_state();
        let ws_id = default_workspace(&state);
        let app = test_app(state);
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", false);
        // Create a few commits so recent_commits is non-empty.
        for i in 1..=3 {
            std::process::Command::new("git")
                .args(["commit", "--allow-empty", "-m", &format!("commit {i}")])
                .current_dir(tmp.path())
                .status()
                .unwrap();
        }
        // Add an unstaged file so dirty_files is non-empty.
        std::fs::write(tmp.path().join("dirty.txt"), "x").unwrap();
        let path = tmp.path().to_str().unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/codebases", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"path":"{}"}}"#, path)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let cid = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/codebases/{}?wid={}", cid, ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body)["data"].clone();
        let status = &detail["git_status"];
        assert_eq!(status["branch"], "main");
        let dirty: Vec<&str> = status["dirty_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            dirty.contains(&"dirty.txt"),
            "dirty_files should include dirty.txt: {:?}",
            dirty
        );
        let commits = status["recent_commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3);
        assert!(commits[0]["hash"].as_str().unwrap().len() >= 7);
        assert_eq!(detail["git_error"], Value::Null);
    }
}
