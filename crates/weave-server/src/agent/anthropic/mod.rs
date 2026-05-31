//! Anthropic Claude provider implementation.
//!
//! Implements [`CodingAgent`] for the Anthropic Messages API with SSE streaming.

mod streaming;
mod types;

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::mpsc;
use tracing::warn;

use crate::agent::{
    CodingAgent, Content, ContentBlock, Message, MessageRequest, ModelInfo, ProviderHealth, Role,
    StreamEvent,
};
use crate::error::ProviderError;

use streaming::{EventConverter, SseLineParser};
use types::{AnthropicMessage, AnthropicRequest, AnthropicToolDef};

const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const MAX_RETRY_ATTEMPTS: u32 = 3;
#[allow(dead_code)] // Used when constructing agents via ProviderRegistry (feat-007)
const CLIENT_TIMEOUT_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// AnthropicAgent
// ---------------------------------------------------------------------------

/// Anthropic Claude agent implementation.
#[allow(dead_code)] // Instantiated by ProviderRegistry (feat-007)
pub struct AnthropicAgent {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
}

impl AnthropicAgent {
    /// Create a new AnthropicAgent.
    ///
    /// `base_url` should be `https://api.anthropic.com` or a compatible proxy.
    #[allow(dead_code)] // Instantiated by ProviderRegistry (feat-007)
    pub fn new(
        base_url: String,
        api_key: String,
        default_model: String,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS))
            .build()
            .map_err(|e| ProviderError::Unreachable(e.to_string()))?;

        Ok(Self {
            client,
            base_url,
            api_key,
            default_model,
        })
    }

    /// Convert domain [`MessageRequest`] into Anthropic wire format.
    fn build_request(&self, request: &MessageRequest) -> AnthropicRequest {
        let messages = request.messages.iter().map(convert_message).collect();

        let tools = request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|t| AnthropicToolDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                })
                .collect()
        });

        AnthropicRequest {
            model: request.model.clone(),
            max_tokens: request.max_tokens,
            system: request.system.clone(),
            messages,
            tools,
            stream: true,
        }
    }

    /// Map HTTP status code and body to [`ProviderError`].
    fn map_http_error(status: u16, body: &str, model: &str) -> ProviderError {
        match status {
            401 => ProviderError::AuthFailed,
            404 => ProviderError::ModelNotFound {
                model: model.to_string(),
            },
            429 => {
                let retry_after_ms = parse_retry_after_from_body(body);
                ProviderError::RateLimited { retry_after_ms }
            }
            500 | 529 => ProviderError::Unreachable(format!("HTTP {status}")),
            _ => ProviderError::Unreachable(format!("HTTP {status}: {body}")),
        }
    }
}

// ---------------------------------------------------------------------------
// CodingAgent implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl CodingAgent for AnthropicAgent {
    fn provider_type(&self) -> &str {
        "anthropic"
    }

    fn display_name(&self) -> &str {
        "Anthropic"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // DB-driven in feat-007; return empty for now
        Ok(vec![])
    }

    async fn send_message(
        &self,
        request: MessageRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let model = request.model.clone();
        let anthropic_req = self.build_request(&request);
        let url = format!("{}/v1/messages", self.base_url);

        // Retry loop for transient errors (429, 500, 529)
        let mut last_error = None;
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            let response = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_API_VERSION)
                .header("content-type", "application/json")
                .json(&anthropic_req)
                .send()
                .await
                .map_err(|e| ProviderError::Unreachable(e.to_string()))?;

            let status = response.status();
            if status.is_success() {
                // Stream the response body
                let (tx, rx) = mpsc::channel::<Result<StreamEvent, ProviderError>>(64);

                tokio::spawn(async move {
                    let mut parser = SseLineParser::new();
                    let mut converter = EventConverter::new();
                    let mut stream = response.bytes_stream();

                    use futures_util::StreamExt as _;

                    while let Some(chunk_result) = stream.next().await {
                        let chunk: bytes::Bytes = match chunk_result {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = tx
                                    .send(Err(ProviderError::StreamInterrupted(e.to_string())))
                                    .await;
                                return;
                            }
                        };

                        let raw_events = parser.feed(chunk.as_ref());
                        for raw in &raw_events {
                            if let Some(anthropic_event) = streaming::interpret_event(raw) {
                                match converter.convert(anthropic_event) {
                                    Ok(Some(stream_events)) => {
                                        for event in stream_events {
                                            if tx.send(Ok(event)).await.is_err() {
                                                return; // receiver dropped
                                            }
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        let _ = tx.send(Err(e)).await;
                                        return;
                                    }
                                }
                            }
                        }
                    }
                });

                return Ok(Box::pin(ReceiverStream::new(rx)));
            }

            // Non-success status
            let body = response.text().await.unwrap_or_default();
            let error = Self::map_http_error(status.as_u16(), &body, &model);

            // Only retry on 429, 500, 529 — all other status codes are permanent failures
            if !matches!(status.as_u16(), 429 | 500 | 529) {
                return Err(error);
            }

            warn!(
                attempt = attempt + 1,
                max_attempts = MAX_RETRY_ATTEMPTS,
                error = %error,
                "Retrying after transient error"
            );

            let delay_ms = match &error {
                ProviderError::RateLimited { retry_after_ms } => *retry_after_ms,
                _ => 1000 * 2u64.pow(attempt),
            };
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            last_error = Some(error);
        }

        Err(last_error.unwrap_or_else(|| ProviderError::Unreachable("max retries exceeded".into())))
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let start = std::time::Instant::now();
        let url = format!("{}/v1/messages", self.base_url);

        let body = serde_json::json!({
            "model": &self.default_model,
            "max_tokens": 1,
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}]
        });

        let result = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(resp) => {
                if resp.status().is_success() {
                    Ok(ProviderHealth {
                        healthy: true,
                        latency_ms,
                        error: None,
                    })
                } else {
                    let status = resp.status().as_u16();
                    let body_text = resp.text().await.unwrap_or_default();
                    let err = Self::map_http_error(status, &body_text, &self.default_model);
                    Ok(ProviderHealth {
                        healthy: false,
                        latency_ms,
                        error: Some(err.to_string()),
                    })
                }
            }
            Err(e) => Ok(ProviderHealth {
                healthy: false,
                latency_ms,
                error: Some(e.to_string()),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// ReceiverStream wrapper (avoids tokio-stream dependency)
// ---------------------------------------------------------------------------

/// Wrapper around `tokio::sync::mpsc::Receiver` that implements `Stream`.
struct ReceiverStream<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> ReceiverStream<T> {
    fn new(rx: mpsc::Receiver<T>) -> Self {
        Self { rx }
    }
}

impl<T> Stream for ReceiverStream<T> {
    type Item = T;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a domain [`Message`] to an Anthropic wire-format message.
fn convert_message(message: &Message) -> AnthropicMessage {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    let content = match &message.content {
        Content::Text(text) => serde_json::Value::String(text.clone()),
        Content::Blocks(blocks) => {
            let values: Vec<serde_json::Value> = blocks.iter().map(convert_content_block).collect();
            serde_json::Value::Array(values)
        }
    };

    AnthropicMessage {
        role: role.to_string(),
        content,
    }
}

/// Convert a domain [`ContentBlock`] to an Anthropic wire-format JSON value.
fn convert_content_block(block: &ContentBlock) -> serde_json::Value {
    match block {
        ContentBlock::Text { text } => serde_json::json!({
            "type": "text",
            "text": text,
        }),
        ContentBlock::ToolUse { id, name, input } => serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        }),
        ContentBlock::Thinking { text } => serde_json::json!({
            "type": "thinking",
            "thinking": text,
        }),
    }
}

/// Parse retry-after hint from error body. Defaults to 1000ms.
fn parse_retry_after_from_body(body: &str) -> u64 {
    // Anthropic may include retry_after in the error JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(ms) = json
            .get("error")
            .and_then(|e| e.get("retry_after_ms"))
            .and_then(|v| v.as_u64())
        {
            return ms;
        }
    }
    1000 // default
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolDefinition;

    #[test]
    fn test_convert_simple_text_message() {
        let msg = Message {
            role: Role::User,
            content: Content::Text("Hello".into()),
        };
        let wire = convert_message(&msg);
        assert_eq!(wire.role, "user");
        assert_eq!(wire.content, serde_json::json!("Hello"));
    }

    #[test]
    fn test_convert_blocks_message() {
        let msg = Message {
            role: Role::Assistant,
            content: Content::Blocks(vec![
                ContentBlock::Text {
                    text: "Let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/tmp/test"}),
                },
            ]),
        };
        let wire = convert_message(&msg);
        assert_eq!(wire.role, "assistant");
        let arr = wire.content.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "tool_use");
        assert_eq!(arr[1]["id"], "tu_1");
    }

    #[test]
    fn test_convert_tool_result_message() {
        let msg = Message {
            role: Role::User,
            content: Content::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "file contents".into(),
            }]),
        };
        let wire = convert_message(&msg);
        let arr = wire.content.as_array().unwrap();
        assert_eq!(arr[0]["type"], "tool_result");
        assert_eq!(arr[0]["tool_use_id"], "tu_1");
    }

    #[test]
    fn test_build_request_with_tools() {
        let agent = AnthropicAgent::new(
            "https://api.anthropic.com".into(),
            "sk-test".into(),
            "claude-sonnet-4-20250514".into(),
        )
        .unwrap();

        let request = MessageRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![Message {
                role: Role::User,
                content: Content::Text("Hello".into()),
            }],
            system: Some("You are helpful".into()),
            max_tokens: 1024,
            tools: Some(vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
        };

        let wire = agent.build_request(&request);
        assert_eq!(wire.model, "claude-sonnet-4-20250514");
        assert_eq!(wire.max_tokens, 1024);
        assert!(wire.stream);
        assert!(wire.system.is_some());
        assert!(wire.tools.is_some());
        assert_eq!(wire.tools.as_ref().unwrap().len(), 1);
        assert_eq!(wire.messages.len(), 1);
    }

    #[test]
    fn test_map_http_error_401() {
        let err = AnthropicAgent::map_http_error(401, "{}", "model");
        assert!(matches!(err, ProviderError::AuthFailed));
    }

    #[test]
    fn test_map_http_error_404() {
        let err = AnthropicAgent::map_http_error(404, "{}", "claude-sonnet");
        assert!(
            matches!(err, ProviderError::ModelNotFound { ref model } if model == "claude-sonnet")
        );
    }

    #[test]
    fn test_map_http_error_429() {
        let err = AnthropicAgent::map_http_error(429, "{}", "model");
        assert!(matches!(err, ProviderError::RateLimited { .. }));
    }

    #[test]
    fn test_map_http_error_500() {
        let err = AnthropicAgent::map_http_error(500, "{}", "model");
        assert!(matches!(err, ProviderError::Unreachable(_)));
    }

    #[test]
    fn test_map_http_error_529() {
        let err = AnthropicAgent::map_http_error(529, "{}", "model");
        assert!(matches!(err, ProviderError::Unreachable(_)));
    }

    #[test]
    fn test_parse_retry_after_from_body() {
        let body = r#"{"error":{"type":"overloaded","retry_after_ms":5000}}"#;
        assert_eq!(parse_retry_after_from_body(body), 5000);
    }

    #[test]
    fn test_parse_retry_after_default() {
        assert_eq!(parse_retry_after_from_body("{}"), 1000);
    }

    #[test]
    fn test_list_models_returns_empty() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let agent = AnthropicAgent::new(
                "https://api.anthropic.com".into(),
                "sk-test".into(),
                "claude-sonnet-4-20250514".into(),
            )
            .unwrap();
            let models = agent.list_models().await.unwrap();
            assert!(models.is_empty());
        });
    }

    /// Verification test for feat-006: HTTP status codes map to correct ProviderError variants.
    #[test]
    fn test_anthropic_error_mapping() {
        // 401 -> AuthFailed
        let err = AnthropicAgent::map_http_error(401, r#"{"error":{"type":"auth_error"}}"#, "m");
        assert!(
            matches!(err, ProviderError::AuthFailed),
            "401 should map to AuthFailed"
        );

        // 404 -> ModelNotFound
        let err = AnthropicAgent::map_http_error(404, "{}", "claude-sonnet");
        assert!(
            matches!(err, ProviderError::ModelNotFound { ref model } if model == "claude-sonnet"),
            "404 should map to ModelNotFound"
        );

        // 429 -> RateLimited (with retry_after_ms from body)
        let body = r#"{"error":{"type":"overloaded","retry_after_ms":5000}}"#;
        let err = AnthropicAgent::map_http_error(429, body, "m");
        assert!(
            matches!(
                err,
                ProviderError::RateLimited {
                    retry_after_ms: 5000
                }
            ),
            "429 should map to RateLimited with retry_after_ms"
        );

        // 429 -> RateLimited (default retry_after_ms)
        let err = AnthropicAgent::map_http_error(429, "{}", "m");
        assert!(
            matches!(
                err,
                ProviderError::RateLimited {
                    retry_after_ms: 1000
                }
            ),
            "429 without retry_after_ms should default to 1000"
        );

        // 500 -> Unreachable (retryable)
        let err = AnthropicAgent::map_http_error(500, "{}", "m");
        assert!(
            matches!(err, ProviderError::Unreachable(_)),
            "500 should map to Unreachable"
        );

        // 529 -> Unreachable (retryable)
        let err = AnthropicAgent::map_http_error(529, "{}", "m");
        assert!(
            matches!(err, ProviderError::Unreachable(_)),
            "529 should map to Unreachable"
        );

        // 400 -> Unreachable (non-retryable, bad request)
        let err = AnthropicAgent::map_http_error(400, "{}", "m");
        assert!(
            matches!(err, ProviderError::Unreachable(_)),
            "400 should map to Unreachable"
        );
    }
}
