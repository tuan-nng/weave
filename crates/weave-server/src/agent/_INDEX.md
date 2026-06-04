# agent/ — AI Provider Abstraction

The `CodingAgent` trait abstracts LLM provider capabilities. Anthropic is the first (and currently only) implementation. `ProviderRegistry` manages agent lifecycle.

## Files

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 12KB | `CodingAgent` trait — `send_message()`, `list_models()`, `health_check()`. Core types: `StreamEvent` (tagged enum: ContentDelta, ToolUse, ThinkingDelta, MessageStop, Error, etc.), `StopReason`, `MessageRequest`, `Message`, `Content`, `ContentBlock`, `Role`, `ToolDefinition`, `ModelInfo`, `ProviderHealth`. All Send+Sync. |
| `registry.rs` | 8KB | `ProviderRegistry` — creates agents from DB provider configs. `ProviderConfig` wraps `agent_type` + config JSON. Add/remove/get agents. Load from DB on startup. |
| `anthropic/mod.rs` | 19KB | `AnthropicAgent` — sends requests to Anthropic Messages API, converts responses to `StreamEvent` items. Error mapping (HTTP status → AppError), retry-after parsing, request building with tools. |
| `anthropic/streaming.rs` | 20KB | SSE parsing for Anthropic streaming responses — converts Anthropic's SSE event stream into `StreamEvent` items via `ReceiverStream`. |
| `anthropic/types.rs` | 4KB | Anthropic-specific request/response types — message shapes, content block structures. |

## Key Patterns

- `CodingAgent` trait uses `#[async_trait]` + returns `Box<dyn Stream<Item = StreamEvent> + Send + Unpin>`
- `StreamEvent` is the universal streaming contract — all providers emit the same event types
- `ProviderRegistry` created at startup, loaded from DB, injected into `AppState`
- Agent lifecycle: `registry.create_agent(config)` → `agent.send_message(request)` → consume stream → drop

## Connections

- **Used by:** `service/sessions.rs` (prompt execution), `service/kanban.rs` (auto-trigger sessions)
- **Depends on:** `error.rs` (AppError, ProviderError), `store/providers.rs` (provider configs)
- **Trait impl:** `AnthropicAgent` in `anthropic/` is the only current implementation
