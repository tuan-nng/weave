pub mod codebases;
pub mod health;
pub mod kanban;
pub mod providers;
pub mod responses;
pub mod sessions;
pub mod specialists;
pub mod static_assets;
pub mod traces;
pub mod workspaces;

use axum::routing::get;
use axum::Router;
use health::ServerStartTime;

use crate::a2a;
use crate::AppState;

/// Build the API router with all routes.
pub fn router(state: AppState, start_time: ServerStartTime) -> Router {
    Router::new()
        .route("/api/health", get(health::health_check))
        // A2A Agent Card (public, no auth per A2A spec)
        .route("/.well-known/agent.json", get(a2a::agent_card::agent_card))
        // A2A protocol endpoints
        .nest("/api/a2a", a2a::router())
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
            get(providers::list_provider_models).post(providers::refresh_provider_models),
        )
        // Specialist routes
        .route("/api/specialists", get(specialists::list_specialists))
        // Session routes
        .route(
            "/api/workspaces/{wid}/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        // F-14: awaiting-input sessions for the global nav badge.
        .route(
            "/api/workspaces/{wid}/sessions/awaiting-input",
            get(sessions::list_awaiting_input_sessions),
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
        .route(
            "/api/sessions/{sid}/trace/tools",
            get(traces::get_session_tool_calls),
        )
        // Kanban routes
        .route(
            "/api/workspaces/{wid}/boards",
            get(kanban::list_boards).post(kanban::create_board),
        )
        .route(
            "/api/workspaces/{wid}/boards/{id}",
            get(kanban::get_board)
                .patch(kanban::update_board)
                .delete(kanban::delete_board),
        )
        .route(
            "/api/workspaces/{wid}/boards/{bid}/columns",
            axum::routing::post(kanban::create_column),
        )
        .route("/api/columns/{id}", axum::routing::patch(kanban::update_column))
        .route(
            "/api/workspaces/{wid}/boards/{bid}/cards",
            axum::routing::post(kanban::create_card),
        )
        .route(
            "/api/tasks/{id}",
            axum::routing::patch(kanban::update_task).delete(kanban::delete_task),
        )
        .route("/api/boards/{bid}/stream", get(kanban::board_stream))
        // Unbound tasks (feat-053): workspace-wide list of active tasks
        // not bound to a session. New-session wizard Step 4 input. The
        // `?unbound=true` filter is required; the handler returns 400
        // for missing/false. Mounted at /api/workspaces/{wid}/tasks to
        // mirror the boards / codebases / sessions workspace-scope
        // pattern.
        .route(
            "/api/workspaces/{wid}/tasks",
            get(kanban::list_unbound_tasks),
        )
        // Codebase routes (feat-032). list/create are workspace-scoped;
        // get/delete are at the top level and take `?wid=` for the
        // cross-workspace 404 guard.
        .route(
            "/api/workspaces/{wid}/codebases",
            get(codebases::list_codebases).post(codebases::create_codebase),
        )
        .route(
            "/api/codebases/{id}",
            get(codebases::get_codebase).delete(codebases::delete_codebase),
        )
        .layer(axum::Extension(state))
        .layer(axum::Extension(start_time))
        .fallback_service(static_assets::spa_service())
}
