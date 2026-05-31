pub mod health;
pub mod providers;
pub mod responses;
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
        .layer(axum::Extension(state))
        .layer(axum::Extension(start_time))
}
