//! Agent Card handler — `GET /.well-known/agent.json`.
//!
//! Serves a server-level Agent Card describing the Weave server's
//! A2A capabilities. Built dynamically from the loaded specialists
//! (as A2A skills) and the server's host information.

use axum::http::HeaderMap;
use axum::{Extension, Json};

use super::types::*;
use crate::api::health::ServerStartTime;
use crate::AppState;

/// `GET /.well-known/agent.json`
///
/// Public (no auth required per A2A spec). Returns a dynamic
/// Agent Card describing this Weave server's capabilities.
pub async fn agent_card(
    Extension(state): Extension<AppState>,
    Extension(_start_time): Extension<ServerStartTime>,
    headers: HeaderMap,
) -> Json<AgentCard> {
    // Build skills from loaded specialists
    let skills: Vec<AgentSkill> = state
        .specialists
        .all()
        .iter()
        .map(|s| AgentSkill {
            id: s.name.to_lowercase().replace(' ', "-"),
            name: s.name.clone(),
            description: s.description.clone(),
            tags: s.tags.clone(),
        })
        .collect();

    // Resolve the server's URL from the Host header, falling back
    // to localhost for direct connections.
    let host = headers
        .get("Host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:3000");
    let url = format!("http://{host}");

    Json(AgentCard {
        name: "Weave".into(),
        description: "Multi-agent coordination platform with AI coding agent sessions, kanban-driven workflows, and A2A protocol support.".into(),
        url,
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: false,
        },
        skills,
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
    })
}
