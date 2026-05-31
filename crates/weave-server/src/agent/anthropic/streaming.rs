//! Anthropic SSE parser and StreamEvent conversion.
//!
//! Parses the raw byte stream from Anthropic's streaming API into
//! [`StreamEvent`] items. Handles the SSE protocol manually (no external crate).

use std::collections::HashMap;

use crate::agent::{StopReason, StreamEvent};
use crate::error::ProviderError;

use super::types::{
    AnthropicSseEvent, ContentBlock, ContentBlockDeltaData, ContentBlockStartData, Delta,
    MessageDeltaData,
};

// ---------------------------------------------------------------------------
// SSE line parser
// ---------------------------------------------------------------------------

/// Parsed raw SSE event before type interpretation.
#[derive(Debug)]
pub(crate) struct RawSseEvent {
    pub event_type: String,
    pub data: String,
}

/// State machine that accumulates bytes and emits complete SSE events.
///
/// Feed chunks via [`SseLineParser::feed`]; it returns any complete events
/// found in the chunk. Handles partial lines across chunk boundaries.
pub(crate) struct SseLineParser {
    current_event: String,
    current_data: String,
    line_buffer: String,
}

impl SseLineParser {
    pub fn new() -> Self {
        Self {
            current_event: String::new(),
            current_data: String::new(),
            line_buffer: String::new(),
        }
    }

    /// Feed a chunk of bytes. Returns any complete SSE events found.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<RawSseEvent> {
        let text = String::from_utf8_lossy(chunk);
        let mut events = Vec::new();

        for ch in text.chars() {
            if ch == '\n' {
                let raw = std::mem::take(&mut self.line_buffer);
                let line = raw.trim_end_matches('\r');
                if line.is_empty() {
                    // Blank line = event boundary
                    if !self.current_event.is_empty() || !self.current_data.is_empty() {
                        events.push(RawSseEvent {
                            event_type: std::mem::take(&mut self.current_event),
                            data: std::mem::take(&mut self.current_data),
                        });
                    }
                } else if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim_start();
                    if self.current_data.is_empty() {
                        self.current_data = data.to_string();
                    } else {
                        self.current_data.push('\n');
                        self.current_data.push_str(data);
                    }
                } else if let Some(event) = line.strip_prefix("event:") {
                    self.current_event = event.trim().to_string();
                }
                // Lines starting with ':' are comments — ignore
            } else {
                self.line_buffer.push(ch);
            }
        }

        events
    }
}

// ---------------------------------------------------------------------------
// Event interpretation
// ---------------------------------------------------------------------------

/// Interpret a raw SSE event into a typed Anthropic event.
pub(crate) fn interpret_event(raw: &RawSseEvent) -> Option<AnthropicSseEvent> {
    match raw.event_type.as_str() {
        "message_start" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::MessageStart),
        "content_block_start" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::ContentBlockStart),
        "content_block_delta" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::ContentBlockDelta),
        "content_block_stop" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::ContentBlockStop),
        "message_delta" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::MessageDelta),
        "message_stop" => Some(AnthropicSseEvent::MessageStop),
        "ping" => Some(AnthropicSseEvent::Ping),
        "error" => serde_json::from_str(&raw.data)
            .ok()
            .map(AnthropicSseEvent::Error),
        _ => None, // Unknown event type — ignore
    }
}

// ---------------------------------------------------------------------------
// Conversion: AnthropicSseEvent → StreamEvent
// ---------------------------------------------------------------------------

/// Stateful converter that maps Anthropic SSE events to [`StreamEvent`] items.
///
/// Maintains minimal state across events: the current content block's tool_use
/// ID (needed to route `input_json_delta` chunks to the correct `ToolUseDelta`).
pub(crate) struct EventConverter {
    /// Tool use ID for the current content block (if it's a tool_use block).
    current_tool_id: Option<String>,
    /// Map from content block index to tool_use ID.
    tool_ids: HashMap<usize, String>,
}

impl EventConverter {
    pub fn new() -> Self {
        Self {
            current_tool_id: None,
            tool_ids: HashMap::new(),
        }
    }

    /// Convert a single Anthropic SSE event into zero or more [`StreamEvent`]s.
    ///
    /// Returns `Ok(None)` for events that produce no output (message_start, ping, etc.).
    /// Returns `Ok(Some(vec))` for events that produce StreamEvent items.
    /// Returns `Err` for error events.
    pub fn convert(
        &mut self,
        event: AnthropicSseEvent,
    ) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        match event {
            AnthropicSseEvent::MessageStart(_) => Ok(None),
            AnthropicSseEvent::ContentBlockStart(data) => self.convert_block_start(data),
            AnthropicSseEvent::ContentBlockDelta(data) => self.convert_block_delta(data),
            AnthropicSseEvent::ContentBlockStop(_) => {
                self.current_tool_id = None;
                Ok(None)
            }
            AnthropicSseEvent::MessageDelta(data) => self.convert_message_delta(data),
            AnthropicSseEvent::MessageStop => Ok(None),
            AnthropicSseEvent::Ping => Ok(None),
            AnthropicSseEvent::Error(data) => {
                Err(ProviderError::StreamInterrupted(data.error.message))
            }
        }
    }

    fn convert_block_start(
        &mut self,
        data: ContentBlockStartData,
    ) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        match data.content_block {
            ContentBlock::ToolUse { id, name } => {
                self.tool_ids.insert(data.index, id.clone());
                self.current_tool_id = Some(id.clone());
                Ok(Some(vec![StreamEvent::ToolUseStart {
                    id,
                    name,
                    input: serde_json::json!({}),
                }]))
            }
            _ => Ok(None),
        }
    }

    fn convert_block_delta(
        &mut self,
        data: ContentBlockDeltaData,
    ) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        match data.delta {
            Delta::TextDelta { text } => Ok(Some(vec![StreamEvent::TextDelta { text }])),
            Delta::InputJsonDelta { partial_json } => {
                let id = self.tool_ids.get(&data.index).cloned().unwrap_or_default();
                Ok(Some(vec![StreamEvent::ToolUseDelta {
                    id,
                    delta: partial_json,
                }]))
            }
            Delta::ThinkingDelta { thinking } => {
                Ok(Some(vec![StreamEvent::Thinking { text: thinking }]))
            }
        }
    }

    fn convert_message_delta(
        &self,
        data: MessageDeltaData,
    ) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        let stop_reason = data
            .delta
            .stop_reason
            .as_deref()
            .map(map_stop_reason)
            .unwrap_or(StopReason::EndTurn);
        Ok(Some(vec![StreamEvent::Done { stop_reason }]))
    }
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::types::{ContentBlockStopData, ErrorData};
    use super::*;

    fn feed_str(parser: &mut SseLineParser, s: &str) -> Vec<RawSseEvent> {
        parser.feed(s.as_bytes())
    }

    #[test]
    fn test_parse_text_delta() {
        let mut parser = SseLineParser::new();
        let events = feed_str(
            &mut parser,
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "content_block_delta");
    }

    #[test]
    fn test_parse_multiple_events() {
        let mut parser = SseLineParser::new();
        let input = "\
event: message_start\ndata: {\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-20250514\"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n\
event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
event: message_stop\ndata: {}\n\n";
        let events = feed_str(&mut parser, input);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_type, "message_start");
        assert_eq!(events[1].event_type, "content_block_delta");
        assert_eq!(events[2].event_type, "message_delta");
        assert_eq!(events[3].event_type, "message_stop");
    }

    #[test]
    fn test_parse_chunked_delivery() {
        let mut parser = SseLineParser::new();
        let part1 = "event: content_block_delta\ndata: {\"index\":0,";
        let part2 = "\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n";

        let events1 = feed_str(&mut parser, part1);
        assert_eq!(events1.len(), 0);

        let events2 = feed_str(&mut parser, part2);
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].event_type, "content_block_delta");
    }

    #[test]
    fn test_parse_comment_lines() {
        let mut parser = SseLineParser::new();
        let events = feed_str(&mut parser, ": keepalive\n\nevent: ping\ndata: {}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "ping");
    }

    #[test]
    fn test_parse_crlf_line_endings() {
        let mut parser = SseLineParser::new();
        let events = feed_str(
            &mut parser,
            "event: content_block_delta\r\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\r\n\r\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "content_block_delta");
        assert!(events[0].data.contains("Hi"));
    }

    #[test]
    fn test_interpret_known_events() {
        let raw = RawSseEvent {
            event_type: "message_start".into(),
            data: r#"{"message":{"id":"msg_1","model":"claude-sonnet-4-20250514"}}"#.into(),
        };
        let event = interpret_event(&raw);
        assert!(matches!(event, Some(AnthropicSseEvent::MessageStart(_))));
    }

    #[test]
    fn test_interpret_unknown_event() {
        let raw = RawSseEvent {
            event_type: "unknown_future_event".into(),
            data: "{}".into(),
        };
        assert!(interpret_event(&raw).is_none());
    }

    #[test]
    fn test_convert_text_delta() {
        let mut converter = EventConverter::new();
        let event = AnthropicSseEvent::ContentBlockDelta(ContentBlockDeltaData {
            index: 0,
            delta: Delta::TextDelta {
                text: "Hello".into(),
            },
        });
        let result = converter.convert(event).unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            StreamEvent::TextDelta {
                text: "Hello".into()
            }
        );
    }

    #[test]
    fn test_convert_tool_use_lifecycle() {
        let mut converter = EventConverter::new();

        // Tool use start
        let start = AnthropicSseEvent::ContentBlockStart(ContentBlockStartData {
            index: 1,
            content_block: ContentBlock::ToolUse {
                id: "tu_42".into(),
                name: "read_file".into(),
            },
        });
        let result = converter.convert(start).unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], StreamEvent::ToolUseStart { id, .. } if id == "tu_42"));

        // Tool use delta
        let delta = AnthropicSseEvent::ContentBlockDelta(ContentBlockDeltaData {
            index: 1,
            delta: Delta::InputJsonDelta {
                partial_json: r#"{"path":"/"#.into(),
            },
        });
        let result = converter.convert(delta).unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], StreamEvent::ToolUseDelta { id, delta }
            if id == "tu_42" && delta == r#"{"path":"/"#));

        // Block stop
        let stop = AnthropicSseEvent::ContentBlockStop(ContentBlockStopData { index: 1 });
        let result = converter.convert(stop).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_message_delta_end_turn() {
        let mut converter = EventConverter::new();
        let event = AnthropicSseEvent::MessageDelta(MessageDeltaData {
            delta: super::super::types::MessageDeltaInner {
                stop_reason: Some("end_turn".into()),
            },
            usage: None,
        });
        let result = converter.convert(event).unwrap().unwrap();
        assert_eq!(
            result[0],
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    #[test]
    fn test_convert_message_delta_max_tokens() {
        let mut converter = EventConverter::new();
        let event = AnthropicSseEvent::MessageDelta(MessageDeltaData {
            delta: super::super::types::MessageDeltaInner {
                stop_reason: Some("max_tokens".into()),
            },
            usage: None,
        });
        let result = converter.convert(event).unwrap().unwrap();
        assert_eq!(
            result[0],
            StreamEvent::Done {
                stop_reason: StopReason::MaxTokens
            }
        );
    }

    #[test]
    fn test_convert_error() {
        let mut converter = EventConverter::new();
        let event = AnthropicSseEvent::Error(ErrorData {
            error: super::super::types::ErrorDetail {
                error_type: "api_error".into(),
                message: "Something went wrong".into(),
            },
        });
        let result = converter.convert(event);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::StreamInterrupted(msg) => assert_eq!(msg, "Something went wrong"),
            other => panic!("expected StreamInterrupted, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_thinking_delta() {
        let mut converter = EventConverter::new();
        let event = AnthropicSseEvent::ContentBlockDelta(ContentBlockDeltaData {
            index: 0,
            delta: Delta::ThinkingDelta {
                thinking: "let me reason...".into(),
            },
        });
        let result = converter.convert(event).unwrap().unwrap();
        assert_eq!(
            result[0],
            StreamEvent::Thinking {
                text: "let me reason...".into()
            }
        );
    }

    #[test]
    fn test_map_stop_reason_all_variants() {
        assert_eq!(map_stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(map_stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(map_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("unknown"), StopReason::EndTurn);
    }

    #[test]
    fn test_full_sse_lifecycle() {
        let mut parser = SseLineParser::new();
        let mut converter = EventConverter::new();

        let input = "\
event: message_start\ndata: {\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-20250514\"}}\n\n\
event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n\
event: content_block_stop\ndata: {\"index\":0}\n\n\
event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
event: message_stop\ndata: {}\n\n";

        let raw_events = feed_str(&mut parser, input);
        assert_eq!(raw_events.len(), 7);

        let mut stream_events = Vec::new();
        for raw in &raw_events {
            if let Some(event) = interpret_event(raw) {
                if let Some(events) = converter.convert(event).unwrap() {
                    stream_events.extend(events);
                }
            }
        }

        assert_eq!(stream_events.len(), 3); // TextDelta, TextDelta, Done
        assert_eq!(
            stream_events[0],
            StreamEvent::TextDelta {
                text: "Hello".into()
            }
        );
        assert_eq!(
            stream_events[1],
            StreamEvent::TextDelta {
                text: " world".into()
            }
        );
        assert_eq!(
            stream_events[2],
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    /// Verification test for feat-006: SSE parsing produces correct StreamEvent sequence.
    #[test]
    fn test_anthropic_sse_parsing() {
        let mut parser = SseLineParser::new();
        let mut converter = EventConverter::new();

        // Simulate a full Anthropic SSE stream with text + tool use
        let input = "\
event: message_start\ndata: {\"message\":{\"id\":\"msg_test\",\"model\":\"claude-sonnet-4-20250514\"}}\n\n\
event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"I'll read the file.\"}}\n\n\
event: content_block_stop\ndata: {\"index\":0}\n\n\
event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_test\",\"name\":\"read_file\"}}\n\n\
event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"/tmp/test\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"index\":1}\n\n\
event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
event: message_stop\ndata: {}\n\n";

        let raw_events = feed_str(&mut parser, input);
        let mut stream_events = Vec::new();
        for raw in &raw_events {
            if let Some(event) = interpret_event(raw) {
                if let Some(events) = converter.convert(event).unwrap() {
                    stream_events.extend(events);
                }
            }
        }

        // Verify the exact event sequence
        assert_eq!(stream_events.len(), 4);
        assert!(
            matches!(&stream_events[0], StreamEvent::TextDelta { text } if text == "I'll read the file.")
        );
        assert!(
            matches!(&stream_events[1], StreamEvent::ToolUseStart { id, name, .. } if id == "tu_test" && name == "read_file")
        );
        assert!(
            matches!(&stream_events[2], StreamEvent::ToolUseDelta { id, delta }
            if id == "tu_test" && delta.contains("path"))
        );
        assert!(matches!(
            &stream_events[3],
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn
            }
        ));
    }
}
