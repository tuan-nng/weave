//! A2A (Agent-to-Agent) protocol types.
//!
//! These types mirror the A2A protocol spec (v1.0) for REST binding.
//! Only the core subset needed for the server endpoints is defined:
//! Agent Card, SendMessage, GetTask, CancelTask, SubscribeToTask.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent Card
// ---------------------------------------------------------------------------

/// Server-level agent card served at `/.well-known/agent.json`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    pub capabilities: AgentCapabilities,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub streaming: bool,
    #[serde(default)]
    pub push_notifications: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// A2A task — maps to a Weave session.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<A2aMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<ArtifactRef>>,
}

/// A2A task status values — lowercased in JSON per A2A spec.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Submitted,
    Working,
    #[allow(dead_code)]
    InputRequired,
    Completed,
    Failed,
    Canceled,
    #[allow(dead_code)]
    Rejected,
}

impl TaskStatus {
    /// Map a Weave session status to an A2A task status.
    pub fn from_session_status(s: &str) -> Self {
        match s {
            "connecting" => TaskStatus::Submitted,
            "ready" => TaskStatus::Working,
            "completed" => TaskStatus::Completed,
            "cancelled" => TaskStatus::Canceled,
            "error" => TaskStatus::Failed,
            _ => TaskStatus::Submitted,
        }
    }
}

/// A lightweight artifact reference for the task response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// An A2A message with structured parts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aMessage {
    pub role: String,
    pub parts: Vec<Part>,
}

/// A single part of an A2A message.
///
/// Only `text` is handled in v1. File and Data parts are
/// deserialized but skipped during prompt construction.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Part {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "file")]
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    File {
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        data: Option<String>,
    },
    #[serde(rename = "data")]
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    Data {
        #[serde(default)]
        data: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// SendMessage
// ---------------------------------------------------------------------------

/// Request body for `POST /api/a2a/messages`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub message: A2aMessage,
    #[serde(default)]
    pub context_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    /// Optional runtime override (defaults to `anthropic-api`).
    /// Wire form: kebab-case, e.g. `"claude-code"`, `"openai-api"`.
    /// Validated against `mode` by `validate_runtime_mode_compat`
    /// (feat-040).
    #[serde(default)]
    pub runtime_kind: Option<crate::agent::RuntimeKind>,
    /// Optional mode override (defaults to `native`).
    /// Wire form: snake_case, e.g. `"native"`, `"wrapped"`, `"attended"`.
    /// Validated against `runtime_kind` by `validate_runtime_mode_compat`
    /// (feat-040).
    #[serde(default)]
    pub mode: Option<crate::agent::SessionMode>,
}

// ---------------------------------------------------------------------------
// SSE events for task subscription
// ---------------------------------------------------------------------------

/// An SSE event emitted on the task subscription stream.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TaskEvent {
    #[serde(rename_all = "camelCase")]
    TaskStatusUpdate {
        task_id: String,
        status: TaskStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract all text content from an A2A message's parts.
pub fn extract_text_from_parts(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_task_status_from_session_status() {
        assert!(matches!(
            TaskStatus::from_session_status("connecting"),
            TaskStatus::Submitted
        ));
        assert!(matches!(
            TaskStatus::from_session_status("ready"),
            TaskStatus::Working
        ));
        assert!(matches!(
            TaskStatus::from_session_status("completed"),
            TaskStatus::Completed
        ));
        assert!(matches!(
            TaskStatus::from_session_status("cancelled"),
            TaskStatus::Canceled
        ));
        assert!(matches!(
            TaskStatus::from_session_status("error"),
            TaskStatus::Failed
        ));
        assert!(matches!(
            TaskStatus::from_session_status("unknown"),
            TaskStatus::Submitted
        ));
    }

    #[test]
    fn test_a2a_extract_text_from_parts() {
        let parts = vec![
            Part::Text {
                text: "Hello".into(),
            },
            Part::Text {
                text: "World".into(),
            },
        ];
        assert_eq!(extract_text_from_parts(&parts), "Hello\nWorld");
    }

    #[test]
    fn test_a2a_extract_text_skips_file_and_data_parts() {
        let parts = vec![
            Part::Text {
                text: "Only text".into(),
            },
            Part::File {
                mime_type: Some("text/plain".into()),
                data: Some("ignored".into()),
            },
            Part::Data {
                data: serde_json::json!({"key": "value"}),
            },
        ];
        assert_eq!(extract_text_from_parts(&parts), "Only text");
    }

    #[test]
    fn test_a2a_extract_text_empty() {
        assert_eq!(extract_text_from_parts(&[]), "");
    }

    #[test]
    fn test_a2a_task_status_serialization() {
        let json = serde_json::to_string(&TaskStatus::Submitted).unwrap();
        assert_eq!(json, "\"submitted\"");
        let json = serde_json::to_string(&TaskStatus::Working).unwrap();
        assert_eq!(json, "\"working\"");
        let json = serde_json::to_string(&TaskStatus::Completed).unwrap();
        assert_eq!(json, "\"completed\"");
    }

    #[test]
    fn test_a2a_send_message_request_deserialization() {
        let json = r#"{"message":{"role":"user","parts":[{"type":"text","text":"Hello"}]}}"#;
        let req: SendMessageRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message.role, "user");
        assert_eq!(req.message.parts.len(), 1);
        assert!(req.task_id.is_none());
        assert!(req.context_id.is_none());
    }

    #[test]
    fn test_a2a_send_message_request_with_task_id() {
        let json = r#"{"message":{"role":"user","parts":[{"type":"text","text":"Continue"}]},"taskId":"abc-123"}"#;
        let req: SendMessageRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.task_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn test_a2a_part_deserialization() {
        let json = r#"{"type":"text","text":"Hello world"}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(matches!(part, Part::Text { ref text } if text == "Hello world"));
    }
}
