//! Provider abstraction layer.
//!
//! Defines the [`CodingAgent`] trait that all AI provider implementations must
//! satisfy, along with the message and streaming types that form the universal
//! contract between providers and the rest of the system.

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

pub mod anthropic;

// ---------------------------------------------------------------------------
// CodingAgent trait
// ---------------------------------------------------------------------------

/// A coding agent that can hold conversations and execute tools.
///
/// Implementations translate between the provider's wire format and the
/// universal [`StreamEvent`] / [`MessageRequest`] types defined here.
/// The trait is object-safe so it can be used as `Arc<dyn CodingAgent>`.
#[allow(dead_code)] // Will be implemented by AnthropicAgent (feat-006)
#[async_trait]
pub trait CodingAgent: Send + Sync {
    /// Unique provider type identifier (e.g. `"anthropic"`, `"openai"`).
    fn provider_type(&self) -> &str;

    /// Human-readable name shown in the UI.
    fn display_name(&self) -> &str;

    /// Available models for this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Send a message and stream back the response.
    ///
    /// Returns a pinned stream of [`StreamEvent`] items. The stream ends with
    /// either a [`StreamEvent::Done`] or [`StreamEvent::Error`].
    async fn send_message(
        &self,
        request: MessageRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>;

    /// Check if the provider is reachable and credentials are valid.
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError>;
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// Events emitted by a provider during a streaming response.
#[allow(dead_code)] // Will be used by SSE streaming (feat-010)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A chunk of assistant text.
    TextDelta { text: String },
    /// Signals the start of a tool invocation.
    ToolUseStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A chunk of tool-use input JSON (for large inputs streamed incrementally).
    ToolUseDelta { id: String, delta: String },
    /// The result of a completed tool invocation.
    ToolResult { id: String, result: String },
    /// Extended-thinking / chain-of-thought text (provider-dependent).
    Thinking { text: String },
    /// The response is complete.
    Done { stop_reason: StopReason },
    /// An error occurred during streaming.
    Error { message: String },
}

/// Reason the model stopped generating.
#[allow(dead_code)] // Will be used by SSE streaming (feat-010)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn naturally.
    EndTurn,
    /// The model hit the `max_tokens` limit.
    MaxTokens,
    /// The model stopped to request a tool invocation.
    ToolUse,
    /// The request was cancelled by the user or system.
    Cancelled,
}

// ---------------------------------------------------------------------------
// Request / message types
// ---------------------------------------------------------------------------

/// A request to send a message to a provider.
#[allow(dead_code)] // Will be used by AnthropicAgent (feat-006)
#[derive(Debug, Clone)]
pub struct MessageRequest {
    /// Model identifier (e.g. `"claude-sonnet-4-20250514"`).
    pub model: String,
    /// Conversation history including the new user message.
    pub messages: Vec<Message>,
    /// Optional system prompt prepended to the conversation.
    pub system: Option<String>,
    /// Maximum tokens the model may generate.
    pub max_tokens: u32,
    /// Tool definitions available to the model for this request.
    pub tools: Option<Vec<ToolDefinition>>,
}

/// A single message in a conversation.
#[allow(dead_code)] // Will be used by AnthropicAgent (feat-006)
#[derive(Debug, Clone)]
pub struct Message {
    /// Who sent this message.
    pub role: Role,
    /// The message content.
    pub content: Content,
}

/// Conversation participant role.
#[allow(dead_code)] // Will be used by AnthropicAgent (feat-006)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Human user.
    User,
    /// AI assistant.
    Assistant,
}

/// Message content — either plain text or structured blocks.
#[allow(dead_code)] // Will be used by AnthropicAgent (feat-006)
#[derive(Debug, Clone)]
pub enum Content {
    /// Simple text content.
    Text(String),
    /// Structured content blocks (text, tool use, tool results, thinking).
    Blocks(Vec<ContentBlock>),
}

/// A single block within structured message content.
#[allow(dead_code)] // Will be used by AnthropicAgent (feat-006)
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Plain text.
    Text { text: String },
    /// A request from the model to invoke a tool.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result of a tool invocation, returned to the model.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    /// Extended-thinking text.
    Thinking { text: String },
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A tool the model can invoke.
#[allow(dead_code)] // Will be used by tool registry (feat-012)
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    /// Tool name (must match the executor's registered name).
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: serde_json::Value,
}

/// Information about an available model.
#[allow(dead_code)] // Will be used by provider API (feat-007)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier used in API calls.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Maximum context window size in tokens.
    pub context_window: u32,
}

/// Result of a provider health check.
#[allow(dead_code)] // Will be used by provider API (feat-007)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    /// Whether the provider is reachable and credentials are valid.
    pub healthy: bool,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// Error message if unhealthy.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_variants() {
        // Construct every variant to verify the enum compiles and is exhaustive.
        let events = [
            StreamEvent::TextDelta {
                text: "hello".into(),
            },
            StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "/tmp/test"}),
            },
            StreamEvent::ToolUseDelta {
                id: "tu_1".into(),
                delta: r#"{"path":"/tmp"#.into(),
            },
            StreamEvent::ToolResult {
                id: "tu_1".into(),
                result: "file contents".into(),
            },
            StreamEvent::Thinking {
                text: "let me think...".into(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
            StreamEvent::Error {
                message: "something went wrong".into(),
            },
        ];

        assert_eq!(events.len(), 7, "StreamEvent must have exactly 7 variants");
    }

    #[test]
    fn test_stop_reason_variants() {
        let reasons = [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::ToolUse,
            StopReason::Cancelled,
        ];
        assert_eq!(reasons.len(), 4, "StopReason must have exactly 4 variants");
    }

    #[test]
    fn test_stream_event_serde_roundtrip() {
        let event = StreamEvent::ToolUseStart {
            id: "tu_42".into(),
            name: "shell_exec".into(),
            input: serde_json::json!({"command": "ls -la"}),
        };

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: StreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_stream_event_json_tagged() {
        // Verify the serde tag = "type" produces the expected shape.
        let event = StreamEvent::TextDelta { text: "hi".into() };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hi");
    }

    #[test]
    fn test_stop_reason_serde_roundtrip() {
        for reason in [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::ToolUse,
            StopReason::Cancelled,
        ] {
            let json = serde_json::to_string(&reason).expect("serialize");
            let deserialized: StopReason = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(reason, deserialized);
        }
    }

    #[test]
    fn test_role_serde_roundtrip() {
        for role in [Role::User, Role::Assistant] {
            let json = serde_json::to_string(&role).expect("serialize");
            let deserialized: Role = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(role, deserialized);
        }
    }

    #[test]
    fn test_model_info_serde_roundtrip() {
        let info = ModelInfo {
            id: "claude-sonnet-4-20250514".into(),
            name: "Claude Sonnet".into(),
            context_window: 200_000,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let deserialized: ModelInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(info.id, deserialized.id);
        assert_eq!(info.name, deserialized.name);
        assert_eq!(info.context_window, deserialized.context_window);
    }

    #[test]
    fn test_provider_health_serde_roundtrip() {
        let health = ProviderHealth {
            healthy: true,
            latency_ms: 42,
            error: None,
        };
        let json = serde_json::to_string(&health).expect("serialize");
        let deserialized: ProviderHealth = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(health.healthy, deserialized.healthy);
        assert_eq!(health.latency_ms, deserialized.latency_ms);
        assert_eq!(health.error, deserialized.error);
    }

    #[test]
    fn test_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<StreamEvent>();
        assert_send_sync::<StopReason>();
        assert_send_sync::<MessageRequest>();
        assert_send_sync::<Message>();
        assert_send_sync::<Role>();
        assert_send_sync::<Content>();
        assert_send_sync::<ContentBlock>();
        assert_send_sync::<ToolDefinition>();
        assert_send_sync::<ModelInfo>();
        assert_send_sync::<ProviderHealth>();
    }
}
