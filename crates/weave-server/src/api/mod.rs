pub mod health;
pub mod providers;
pub mod responses;
pub mod sessions;
pub mod traces;
pub mod workspaces;

use axum::routing::get;
use axum::Router;
use health::ServerStartTime;

use crate::AppState;

/// Build the API router with all routes.
pub fn router(state: AppState, start_time: ServerStartTime) -> Router {
    Router::new()
        .route("/api/health", get(health::health_check))
        // Workspace routes
        .route(
            "/api/workspaces",
            get(workspaces::list_workspaces).post(workspaces::create_workspace),
        )
        .route(
            "/api/workspaces/{id}",
            get(workspaces::get_workspace)
                .patch(workspaces::update_workspace)
                .delete(workspaces::delete_workspace),
        )
        // Provider routes
        .route(
            "/api/providers",
            get(providers::list_providers).post(providers::create_provider),
        )
        .route(
            "/api/providers/{id}",
            axum::routing::delete(providers::delete_provider),
        )
        .route(
            "/api/providers/{id}/models",
            get(providers::list_provider_models),
        )
        // Session routes
        .route(
            "/api/workspaces/{wid}/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route(
            "/api/sessions/{id}",
            get(sessions::get_session)
                .patch(sessions::update_session_status)
                .delete(sessions::delete_session),
        )
        .route(
            "/api/sessions/{sid}/history",
            get(sessions::get_session_history),
        )
        .route(
            "/api/sessions/{sid}/prompt",
            axum::routing::post(sessions::send_prompt),
        )
        .route(
            "/api/sessions/{sid}/cancel",
            axum::routing::post(sessions::cancel_session),
        )
        .route("/api/sessions/{sid}/stream", get(sessions::session_stream))
        // Trace routes
        .route(
            "/api/sessions/{sid}/trace",
            get(traces::get_session_trace),
        )
        .route(
            "/api/sessions/{sid}/trace/journey",
            get(traces::get_session_journey),
        )
        .route(
            "/api/sessions/{sid}/trace/files",
            get(traces::get_session_file_changes),
        )
        .layer(axum::Extension(state))
        .layer(axum::Extension(start_time))
}
