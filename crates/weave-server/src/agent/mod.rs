//! Provider abstraction layer.
//!
//! Defines the [`CodingAgent`] trait that all AI provider implementations must
//! satisfy, along with the message and streaming types that form the universal
//! contract between providers and the rest of the system.

use std::pin::Pin;
use std::str::FromStr;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, ProviderError};

pub mod anthropic;
pub mod model_cache;
pub mod registry;
pub mod turn_context;

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
    /// `turn` is the per-turn execution context built by
    /// `service::sessions::run_prompt_task`. It carries session /
    /// workspace identity, working directories, the CLI-native resume id
    /// (HTTP runtimes see `None`), the effective permission snapshot,
    /// and the cancellation token. HTTP (`Anthropic`) implementations
    /// ignore most fields today; CLI implementations (feat-043+) will
    /// consume the full struct. See
    /// [`crate::agent::turn_context::TurnContext`].
    ///
    /// Returns a pinned stream of [`StreamEvent`] items. The stream ends with
    /// either a [`StreamEvent::Done`] or [`StreamEvent::Error`].
    async fn send_message(
        &self,
        request: MessageRequest,
        turn: &turn_context::TurnContext,
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
    /// The agent loop hit the configured per-turn iteration cap (feat-037).
    /// The model never produced a final `end_turn`; we stopped after `iterations`
    /// tool rounds and surfaced a final assistant "Sorry, too many tool calls."
    /// message so the user sees something coherent rather than an open stream.
    LoopLimit { iterations: u32 },
}

// ---------------------------------------------------------------------------
// Runtime / mode enums (feat-038)
// ---------------------------------------------------------------------------

/// Which Runtime Tool a session runs on.
///
/// `RuntimeKind` is the discriminator for the `providers.kind` widening
/// (feat-039) and for the per-turn `TurnContext` (feat-041). The wire
/// format is snake_case (the same as the SQL column default), so a
/// round-trip through JSON or SQLite text is lossless.
///
/// `AnthropicApi` is the pre-feat-038 default — every existing row
/// backfills to this value. The full matrix of `runtime_kind` × `mode`
/// compatibility is enforced in feat-040.
#[allow(dead_code)] // Wider wiring lands in feat-039+; surface kept typed here
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    /// The Anthropic HTTP API in native mode (the pre-feat-038 default).
    AnthropicApi,
    /// The OpenAI HTTP API in native mode.
    OpenaiApi,
    /// An OpenAI-compatible HTTP endpoint (configurable `base_url`).
    OpenaiCompatible,
    /// The Claude Code CLI in wrapped mode (Phase 8 — feat-043+).
    ClaudeCode,
    /// The Codex CLI in wrapped mode (Phase 10 — feat-057+).
    Codex,
    /// The OpenCode CLI in wrapped mode (Phase 10 — feat-059+).
    Opencode,
}

impl RuntimeKind {
    /// Stable kebab-case wire form (matches the SQL column default and
    /// the `#[serde(rename_all = "kebab-case")]` JSON shape).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AnthropicApi => "anthropic-api",
            Self::OpenaiApi => "openai-api",
            Self::OpenaiCompatible => "openai-compatible",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

impl Default for RuntimeKind {
    /// Pre-feat-038 default — every backfilled row lands here.
    fn default() -> Self {
        Self::AnthropicApi
    }
}

impl std::fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuntimeKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic-api" => Ok(Self::AnthropicApi),
            "openai-api" => Ok(Self::OpenaiApi),
            "openai-compatible" => Ok(Self::OpenaiCompatible),
            "claude-code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            other => Err(AppError::validation(format!(
                "invalid runtime_kind '{other}', expected one of: \
                 anthropic-api, openai-api, openai-compatible, claude-code, codex, opencode"
            ))),
        }
    }
}

/// How the agent drives a turn.
///
/// `Native` is the pre-feat-038 default — every existing row backfills
/// to this value. `Wrapped` means a CLI subprocess is invoked per
/// turn (Phase 8+). `Attended` is a separate Terminal abstraction
/// (deferred — Phase 11).
#[allow(dead_code)] // Wider wiring lands in feat-040+
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Weave drives the turn via a direct provider call (HTTP, native).
    Native,
    /// Weave drives the turn by spawning a CLI subprocess per turn.
    Wrapped,
    /// A human drives the CLI; Weave only observes. Deferred to Phase 11.
    Attended,
}

impl SessionMode {
    /// Stable snake-case wire form (matches the SQL column default).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wrapped => "wrapped",
            Self::Attended => "attended",
        }
    }
}

impl Default for SessionMode {
    /// Pre-feat-038 default.
    fn default() -> Self {
        Self::Native
    }
}

impl std::fmt::Display for SessionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SessionMode {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "native" => Ok(Self::Native),
            "wrapped" => Ok(Self::Wrapped),
            "attended" => Ok(Self::Attended),
            other => Err(AppError::validation(format!(
                "invalid mode '{other}', expected one of: native, wrapped, attended"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime × mode compatibility (feat-040)
// ---------------------------------------------------------------------------

/// Return the set of `SessionMode` values the given runtime supports.
///
/// Source of truth for the compatibility matrix defined in
/// `docs/road-map/multi-runtime-strategy.md` §4. HTTP runtimes only
/// support `Native`; CLI runtimes only support `Wrapped`. `Attended`
/// is reserved for Phase 11 and is *not* included in any runtime's
/// supported set today — `validate_runtime_mode_compat` rejects every
/// `attended` pairing until Phase 11 lands.
///
/// Exposed publicly so downstream features (feat-046 `PermissionMapper`,
/// feat-053 session-creation wizard) can filter or default against the
/// same set without re-deriving the matrix.
pub fn supported_modes(runtime: RuntimeKind) -> &'static [SessionMode] {
    match runtime {
        // HTTP runtimes: Weave drives the turn via a direct provider call.
        RuntimeKind::AnthropicApi | RuntimeKind::OpenaiApi | RuntimeKind::OpenaiCompatible => {
            &[SessionMode::Native]
        }
        // CLI runtimes: Weave drives the turn by spawning a CLI subprocess.
        RuntimeKind::ClaudeCode | RuntimeKind::Codex | RuntimeKind::Opencode => {
            &[SessionMode::Wrapped]
        }
    }
}

/// Enforce the `RuntimeKind` × `SessionMode` compatibility matrix at
/// session-creation time. Every inbound path (POST
/// `/api/workspaces/:wid/sessions`, A2A POST `/api/a2a/messages`,
/// kanban `try_automate_lane`) routes through
/// `SessionService::create_session`, which calls this after parsing
/// both fields but before any parent-chain or transaction work.
///
/// Returns `Ok(())` for every allowed pair. Returns
/// `AppError::Validation { code: "runtime_mode_incompatible", .. }`
/// for every other combination, with a `message` listing the runtime,
/// the requested mode, and the modes that runtime does support.
pub fn validate_runtime_mode_compat(
    runtime: RuntimeKind,
    mode: SessionMode,
) -> Result<(), AppError> {
    let supported = supported_modes(runtime);
    if supported.contains(&mode) {
        Ok(())
    } else {
        let listed = supported
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(AppError::validation_with_code(
            "runtime_mode_incompatible",
            format!(
                "runtime '{}' does not support mode '{}'; supported modes: [{}]",
                runtime.as_str(),
                mode.as_str(),
                listed,
            ),
        ))
    }
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
        /// Whether the tool reported a failure (validation, missing tool, exec error,
        /// timeout, etc.). Mirrors Anthropic's `is_error` flag on `tool_result` blocks
        /// so the model can react and try a different approach instead of looping
        /// blindly on a tool that will keep failing.
        is_error: bool,
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
            StopReason::LoopLimit { iterations: 8 },
        ];
        assert_eq!(reasons.len(), 5, "StopReason must have exactly 5 variants");
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
            StopReason::LoopLimit { iterations: 8 },
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
        assert_send_sync::<RuntimeKind>();
        assert_send_sync::<SessionMode>();
    }

    // --- feat-038: RuntimeKind / SessionMode ---

    #[test]
    fn test_runtime_kind_variants() {
        let kinds = [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ];
        assert_eq!(kinds.len(), 6, "RuntimeKind must have exactly 6 variants");
    }

    #[test]
    fn test_runtime_kind_default_is_anthropic_api() {
        // The pre-feat-038 default — every backfilled row lands here.
        assert_eq!(RuntimeKind::default(), RuntimeKind::AnthropicApi);
    }

    #[test]
    fn test_runtime_kind_as_str() {
        assert_eq!(RuntimeKind::AnthropicApi.as_str(), "anthropic-api");
        assert_eq!(RuntimeKind::OpenaiApi.as_str(), "openai-api");
        assert_eq!(RuntimeKind::OpenaiCompatible.as_str(), "openai-compatible");
        assert_eq!(RuntimeKind::ClaudeCode.as_str(), "claude-code");
        assert_eq!(RuntimeKind::Codex.as_str(), "codex");
        assert_eq!(RuntimeKind::Opencode.as_str(), "opencode");
    }

    #[test]
    fn test_runtime_kind_from_str_roundtrip() {
        for kind in [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ] {
            let parsed: RuntimeKind = kind.as_str().parse().expect("valid wire form");
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn test_runtime_kind_from_str_rejects_unknown() {
        let err = "not-a-runtime".parse::<RuntimeKind>().unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("invalid runtime_kind"))
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_runtime_kind_serde_roundtrip() {
        // JSON wire form is kebab-case (matches SQL column default).
        for kind in [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize");
            let deserialized: RuntimeKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(deserialized, kind);
        }
        // Spot-check the wire shape.
        let json = serde_json::to_string(&RuntimeKind::AnthropicApi).unwrap();
        assert_eq!(json, "\"anthropic-api\"");
    }

    #[test]
    fn test_session_mode_variants() {
        let modes = [
            SessionMode::Native,
            SessionMode::Wrapped,
            SessionMode::Attended,
        ];
        assert_eq!(modes.len(), 3, "SessionMode must have exactly 3 variants");
    }

    #[test]
    fn test_session_mode_default_is_native() {
        // The pre-feat-038 default — every backfilled row lands here.
        assert_eq!(SessionMode::default(), SessionMode::Native);
    }

    #[test]
    fn test_session_mode_as_str() {
        assert_eq!(SessionMode::Native.as_str(), "native");
        assert_eq!(SessionMode::Wrapped.as_str(), "wrapped");
        assert_eq!(SessionMode::Attended.as_str(), "attended");
    }

    #[test]
    fn test_session_mode_from_str_roundtrip() {
        for mode in [
            SessionMode::Native,
            SessionMode::Wrapped,
            SessionMode::Attended,
        ] {
            let parsed: SessionMode = mode.as_str().parse().expect("valid wire form");
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn test_session_mode_from_str_rejects_unknown() {
        let err = "turbo".parse::<SessionMode>().unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => assert!(msg.contains("invalid mode")),
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_session_mode_serde_roundtrip() {
        // JSON wire form is snake_case (matches SQL column default).
        for mode in [
            SessionMode::Native,
            SessionMode::Wrapped,
            SessionMode::Attended,
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let deserialized: SessionMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(deserialized, mode);
        }
        let json = serde_json::to_string(&SessionMode::Native).unwrap();
        assert_eq!(json, "\"native\"");
    }

    // --- feat-040: runtime × mode compatibility ---

    #[test]
    fn test_supported_modes() {
        // HTTP runtimes support only `native`; CLI runtimes support only
        // `wrapped`. `attended` is reserved for Phase 11 and not in any
        // runtime's supported set today.
        for runtime in [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
        ] {
            assert_eq!(
                supported_modes(runtime),
                &[SessionMode::Native],
                "HTTP runtime {runtime:?} must support only Native"
            );
        }
        for runtime in [
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ] {
            assert_eq!(
                supported_modes(runtime),
                &[SessionMode::Wrapped],
                "CLI runtime {runtime:?} must support only Wrapped"
            );
        }
    }

    #[test]
    fn test_runtime_mode_compat_anthropic_native_ok() {
        assert!(
            validate_runtime_mode_compat(RuntimeKind::AnthropicApi, SessionMode::Native).is_ok()
        );
    }

    #[test]
    fn test_runtime_mode_compat_anthropic_wrapped_rejected() {
        let err = validate_runtime_mode_compat(RuntimeKind::AnthropicApi, SessionMode::Wrapped)
            .expect_err("HTTP runtime + wrapped must be rejected");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "runtime_mode_incompatible");
                assert!(message.contains("anthropic-api"), "msg: {message}");
                assert!(message.contains("wrapped"), "msg: {message}");
                assert!(message.contains("native"), "msg: {message}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_runtime_mode_compat_claude_code_wrapped_ok() {
        assert!(
            validate_runtime_mode_compat(RuntimeKind::ClaudeCode, SessionMode::Wrapped).is_ok()
        );
    }

    #[test]
    fn test_runtime_mode_compat_claude_code_native_rejected() {
        let err = validate_runtime_mode_compat(RuntimeKind::ClaudeCode, SessionMode::Native)
            .expect_err("CLI runtime + native must be rejected");
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "runtime_mode_incompatible");
                assert!(message.contains("claude-code"), "msg: {message}");
                assert!(message.contains("native"), "msg: {message}");
                assert!(message.contains("wrapped"), "msg: {message}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_runtime_mode_compat_attended_rejected_for_now() {
        // Attended is deferred to Phase 11 — every runtime must reject it
        // with the same `runtime_mode_incompatible` code. The HTTP case
        // is the most likely user path (Anthropic API in attended mode
        // is the closest to a real Phase 11 surface).
        for runtime in [
            RuntimeKind::AnthropicApi,
            RuntimeKind::OpenaiApi,
            RuntimeKind::OpenaiCompatible,
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Opencode,
        ] {
            let err = validate_runtime_mode_compat(runtime, SessionMode::Attended)
                .expect_err("attended must be rejected for every runtime");
            match err {
                AppError::Validation { code, message } => {
                    assert_eq!(code, "runtime_mode_incompatible");
                    assert!(message.contains("attended"), "msg: {message}");
                    // No Phase 11 reference in the message (Q3 decision).
                    assert!(
                        !message.contains("phase"),
                        "attended rejection must not reference Phase 11, got: {message}"
                    );
                }
                other => panic!("expected Validation for {runtime:?}, got: {other:?}"),
            }
        }
    }
}
