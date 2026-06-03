//! A2A (Agent-to-Agent) protocol server endpoints.
//!
//! Implements the core A2A subset (v1.0 REST binding):
//! - Agent Card: `GET /.well-known/agent.json`
//! - SendMessage: `POST /api/a2a/messages`
//! - GetTask: `GET /api/a2a/tasks/{id}`
//! - CancelTask: `POST /api/a2a/tasks/{id}/cancel`
//! - SubscribeToTask (SSE): `GET /api/a2a/tasks/{id}/subscribe`
//!
//! Auth is handled per-endpoint via `verify_a2a_token` using the
//! shared `WEAVE_A2A_TOKEN` environment variable.

pub mod agent_card;
pub mod auth;
pub mod messages;
pub mod tasks;
pub mod types;

use axum::routing::{get, post};
use axum::Router;

/// Build the A2A sub-router mounted at `/api/a2a`.
pub fn router() -> Router {
    Router::new()
        .route("/messages", post(messages::send_message))
        .route("/tasks/{id}", get(tasks::get_task))
        .route("/tasks/{id}/cancel", post(tasks::cancel_task))
        .route("/tasks/{id}/subscribe", get(tasks::subscribe_task))
}
