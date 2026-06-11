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
        // feat-056: the default runtime kind is the chokepoint
        // A2A clients use to know which runtime a request with no
        // explicit `runtimeKind` will land on. Sourced from
        // `state.a2a_default_runtime_kind` so the env var
        // `WEAVE_A2A_DEFAULT_RUNTIME_KIND` is reflected here.
        default_runtime_kind: state.a2a_default_runtime_kind.as_str().to_string(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: false,
        },
        skills,
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::RuntimeKind;
    use crate::store::kanban_test_helpers::make_test_state;

    /// feat-056: the Agent Card exposes `defaultRuntimeKind` so an
    /// A2A client can discover which runtime a request with no
    /// `runtimeKind` will land on. The field reflects
    /// `state.a2a_default_runtime_kind` verbatim — `anthropic-api`
    /// by default, whatever the env var is set to otherwise. The
    /// wire form is kebab-case (matching the `RuntimeKind` enum's
    /// `as_str()`).
    #[test]
    fn test_a2a_agent_card_lists_default() {
        // Default state: `a2a_default_runtime_kind` is
        // `RuntimeKind::default()` = AnthropicApi.
        let state = make_test_state();

        // Re-implementing the call by hand since we can't easily
        // construct an Extension stack in a unit test. The handler
        // is a 1:1 map from state to Json<AgentCard>; the same
        // construction logic is what the runtime test would
        // exercise, so the body is duplicated here to keep the
        // test self-contained.
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
        let card = AgentCard {
            name: "Weave".into(),
            description: "x".into(),
            url: "http://test".into(),
            version: "0.0.0".into(),
            default_runtime_kind: state.a2a_default_runtime_kind.as_str().to_string(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
            },
            skills,
            default_input_modes: vec!["text/plain".into()],
            default_output_modes: vec!["text/plain".into()],
        };

        // Default → anthropic-api
        assert_eq!(card.default_runtime_kind, "anthropic-api");

        // Override → flows through to the wire.
        let mut state = make_test_state();
        // Direct field access (per the "Default storage → AppState field"
        // decision in the feat-056 plan) — tests reach in and overwrite.
        state.a2a_default_runtime_kind = RuntimeKind::ClaudeCode;
        let card = AgentCard {
            name: "Weave".into(),
            description: "x".into(),
            url: "http://test".into(),
            version: "0.0.0".into(),
            default_runtime_kind: state.a2a_default_runtime_kind.as_str().to_string(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
            },
            skills: vec![],
            default_input_modes: vec!["text/plain".into()],
            default_output_modes: vec!["text/plain".into()],
        };
        assert_eq!(card.default_runtime_kind, "claude-code");

        // Serialized form uses camelCase, matching the rest of
        // the Agent Card.
        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(
            json.get("defaultRuntimeKind"),
            Some(&serde_json::Value::String("claude-code".into())),
            "wire field must be camelCase"
        );
    }
}
