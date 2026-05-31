//! Anthropic Messages API wire format types.
//!
//! These types mirror the Anthropic JSON schema exactly — they exist only for
//! serialization/deserialization and carry no domain semantics.
#![allow(dead_code)] // All fields used by serde; some not read in application code

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Top-level request body for `POST /v1/messages`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicToolDef>>,
    pub stream: bool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicMessage {
    pub role: String,
    /// String for simple text, array for structured content blocks.
    pub content: serde_json::Value,
}

/// Tool definition sent to the API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Non-streaming response (used by health_check)
// ---------------------------------------------------------------------------

/// Response from a non-streaming `POST /v1/messages` call.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    pub id: String,
    pub stop_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE event types (what Anthropic sends back in a streaming response)
// ---------------------------------------------------------------------------

/// Top-level SSE event data — tagged by `event:` field, not `type`.
/// We parse the `data:` JSON into these structs based on the `event:` value.
#[derive(Debug, Deserialize)]
pub(crate) struct MessageStartData {
    pub message: MessageInfo,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageInfo {
    pub id: String,
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockStartData {
    pub index: usize,
    pub content_block: ContentBlock,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockDeltaData {
    pub index: usize,
    pub delta: Delta,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)] // Names match Anthropic's wire format
pub(crate) enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockStopData {
    pub index: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaData {
    pub delta: MessageDeltaInner,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaInner {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Usage {
    pub output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorData {
    pub error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// High-level parsed event
// ---------------------------------------------------------------------------

/// A fully parsed Anthropic SSE event.
#[derive(Debug)]
pub(crate) enum AnthropicSseEvent {
    MessageStart(MessageStartData),
    ContentBlockStart(ContentBlockStartData),
    ContentBlockDelta(ContentBlockDeltaData),
    ContentBlockStop(ContentBlockStopData),
    MessageDelta(MessageDeltaData),
    MessageStop,
    Ping,
    Error(ErrorData),
}
