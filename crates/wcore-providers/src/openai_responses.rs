//! OpenAI **Responses API** (`POST /v1/responses`) request/stream adapter.
//!
//! OpenAI's `gpt-5*` family is rejected at `/v1/chat/completions` with
//! `unsupported_api_for_model`; it is served ONLY by the Responses API, which
//! uses a different request body and a different streaming event shape than
//! Chat Completions. This module mirrors that surface and translates it back
//! into wayland-core's provider-neutral [`LlmRequest`] / [`LlmEvent`] types so
//! the engine is unchanged.
//!
//! The chat-vs-responses routing decision lives in
//! [`crate::openai_compat::responses_api_override`] (a per-model family
//! predicate with a `ProviderCompat` override); `OpenAIProvider::stream`
//! consults it to pick the endpoint + the parser in this module.
//!
//! ## Reference
//!
//! The request body shape and the streaming event taxonomy below were mirrored
//! from OpenClaw's mature TypeScript implementation:
//!
//! * `openclaw/src/llm/providers/openai-responses.ts` — top-level request
//!   params (`model`, `input`, `stream: true`, `max_output_tokens`,
//!   `reasoning`, `tools`).
//! * `openclaw/src/llm/providers/openai-responses-shared.ts` —
//!   `convertResponsesMessages` (the `input` item shapes:
//!   `message`/`function_call`/`function_call_output`, with `input_text` /
//!   `output_text` content parts and a `developer`/`system` instruction
//!   message) and `processResponsesStream` (the event handlers below).
//! * `openclaw/src/llm/providers/openai-responses-tools.ts` —
//!   `convertResponsesTools` (the flat `{type:"function", name, description,
//!   parameters}` Responses tool format, distinct from Chat Completions'
//!   nested `{type:"function", function:{...}}`).
//!
//! ## Request body (`/v1/responses`)
//!
//! ```jsonc
//! {
//!   "model": "gpt-5",
//!   "instructions": "<system prompt>",   // system prompt goes here, NOT in input
//!   "input": [                            // typed items, NOT chat "messages"
//!     { "type": "message", "role": "user",
//!       "content": [{ "type": "input_text", "text": "..." }] },
//!     { "type": "message", "role": "assistant",
//!       "content": [{ "type": "output_text", "text": "..." }] },
//!     { "type": "function_call", "call_id": "call_x",
//!       "name": "read", "arguments": "{...}" },
//!     { "type": "function_call_output", "call_id": "call_x", "output": "..." }
//!   ],
//!   "tools": [                            // flat function schema (no nesting)
//!     { "type": "function", "name": "read",
//!       "description": "...", "parameters": { ... } }
//!   ],
//!   "max_output_tokens": 4096,
//!   "reasoning": { "effort": "medium" },  // only for reasoning models
//!   "stream": true
//! }
//! ```
//!
//! ## Streaming events (SSE `data:` frames, each a typed JSON object)
//!
//! * `response.created` — opening frame (carries `response.id`).
//! * `response.output_item.added` — a new output item starts
//!   (`message` / `reasoning` / `function_call`).
//! * `response.output_text.delta` — assistant text delta → [`LlmEvent::TextDelta`].
//! * `response.reasoning_summary_text.delta` /
//!   `response.reasoning_text.delta` — reasoning delta → [`LlmEvent::ThinkingDelta`].
//! * `response.function_call_arguments.delta` — tool-call argument JSON delta
//!   (accumulated, not emitted per-delta).
//! * `response.function_call_arguments.done` — final argument JSON for the call.
//! * `response.output_item.done` — an output item finalized; for a
//!   `function_call` item this is where we emit [`LlmEvent::ToolUse`] (failing
//!   closed on malformed argument JSON).
//! * `response.completed` — terminal success; carries `response.status` +
//!   `response.usage` → [`LlmEvent::Done`].
//! * `response.failed` / `error` — terminal failure → [`LlmEvent::Error`].

use serde_json::{Value, json};

use crate::openai_tool_name::{decode_tool_name, encode_tool_name};
use wcore_config::compat::ProviderCompat;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::tool::{ToolDef, truncate_deferred_description};

/// Build the `/v1/responses` request body from a provider-neutral
/// [`LlmRequest`]. Mirrors `buildParams` + `convertResponsesMessages` +
/// `convertResponsesTools` from the OpenClaw references in the module docs.
pub(crate) fn build_responses_body(request: &LlmRequest, _compat: &ProviderCompat) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": build_input(&request.messages),
        "stream": true,
        // `store: false` keeps OpenAI from persisting the response server-side;
        // wayland-core manages its own history. Mirrors OpenClaw's default.
        "store": false,
        "max_output_tokens": request.max_tokens,
    });

    // System prompt rides the dedicated `instructions` field, NOT an input
    // item (OpenClaw pushes a developer/system message; the `instructions`
    // field is the documented equivalent and avoids ordering ambiguity).
    if !request.system.is_empty() {
        body["instructions"] = json!(request.system);
    }

    if !request.tools.is_empty() {
        body["tools"] = json!(build_responses_tools(&request.tools));
    }

    // Reasoning effort: only meaningful for reasoning models (gpt-5 family is
    // reasoning-capable). The classic chat families never reach this path.
    if let Some(effort) = &request.reasoning_effort {
        body["reasoning"] = json!({ "effort": effort });
    }

    body
}

/// Convert the conversation history into the Responses `input` array of typed
/// items. Mirrors `convertResponsesMessages` in
/// `openai-responses-shared.ts`.
fn build_input(messages: &[Message]) -> Vec<Value> {
    let mut input: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::User => push_user_items(&mut input, msg),
            Role::Assistant => push_assistant_items(&mut input, msg),
            Role::Tool => push_tool_result_items(&mut input, msg),
            // System messages are folded into `instructions` upstream.
            Role::System => {}
        }
    }

    // Drop orphaned `function_call_output` items. The Responses API 400s with
    // "No tool call found for function call output" on any function_call_output
    // whose `call_id` has no matching `function_call` item in the same input
    // set — the same failure the Chat Completions path closes via
    // `clean_orphaned_tool_results` (FerroxLabs/wayland#85). An orphan arises
    // when history trimming drops the parent assistant `function_call` while
    // keeping its result. Stripping it is strictly correct; an orphaned output
    // is unconditionally invalid to send.
    clean_orphaned_function_call_outputs(&mut input);

    input
}

/// Remove `function_call_output` items whose `call_id` matches no
/// `function_call` item in the same input set — the Responses-shape
/// counterpart to `openai.rs::clean_orphaned_tool_results`.
fn clean_orphaned_function_call_outputs(input: &mut Vec<Value>) {
    use std::collections::HashSet;

    let called_ids: HashSet<String> = input
        .iter()
        .filter(|item| item["type"].as_str() == Some("function_call"))
        .filter_map(|item| item["call_id"].as_str().map(String::from))
        .collect();

    input.retain(|item| {
        if item["type"].as_str() == Some("function_call_output")
            && let Some(id) = item["call_id"].as_str()
        {
            // Keep only outputs whose call survives in the input set.
            called_ids.contains(id)
        } else {
            // Non-output items, and any malformed output without a call_id,
            // are out of scope for this pass.
            true
        }
    });
}

/// A user message becomes either a `message` item with `input_text` content,
/// or — when it carries tool results (wayland-core threads tool results as
/// user-role `ToolResult` blocks) — one `function_call_output` item per result.
fn push_user_items(input: &mut Vec<Value>, msg: &Message) {
    let has_tool_results = msg
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

    if has_tool_results {
        push_tool_result_items(input, msg);
        return;
    }

    let text: String = collect_text(msg);
    if text.is_empty() {
        return;
    }
    input.push(json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": text }],
    }));
}

/// An assistant message lowers to: an optional `message` item (its text as an
/// `output_text` content part) followed by one `function_call` item per tool
/// call. Reasoning blocks are dropped on the way back to the model — the
/// Responses API pairs reasoning items with encrypted ids we do not persist,
/// and re-sending unpaired reasoning items triggers validation errors. (Text +
/// tool calls are the load-bearing round-trip; matches the chat path, which
/// also does not re-send `reasoning_content` unless a thinking turn requires
/// it.)
fn push_assistant_items(input: &mut Vec<Value>, msg: &Message) {
    let text: String = msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    if !text.is_empty() {
        input.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }],
            "status": "completed",
        }));
    }

    for block in &msg.content {
        if let ContentBlock::ToolUse {
            id,
            name,
            input: args,
            ..
        } = block
        {
            input.push(json!({
                "type": "function_call",
                "call_id": id,
                "name": encode_tool_name(name),
                "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string()),
            }));
        }
    }
}

/// Tool results (whether carried on a `Tool`-role message or as `ToolResult`
/// blocks on a user message) become `function_call_output` items keyed by
/// `call_id`. Mirrors the `toolResult` arm of `convertResponsesMessages`.
fn push_tool_result_items(input: &mut Vec<Value>, msg: &Message) {
    for block in &msg.content {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
        {
            input.push(json!({
                "type": "function_call_output",
                "call_id": tool_use_id,
                "output": content,
            }));
        }
    }
}

fn collect_text(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert tool definitions to the FLAT Responses function-tool schema:
/// `{ "type": "function", "name", "description", "parameters" }`. Note this is
/// DIFFERENT from Chat Completions' nested `{ "type": "function", "function":
/// { "name", ... } }`. Mirrors `convertResponsesTools` in
/// `openai-responses-tools.ts`.
fn build_responses_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            if t.deferred {
                let short_desc = truncate_deferred_description(&t.description);
                json!({
                    "type": "function",
                    "name": encode_tool_name(&t.name),
                    "description": format!(
                        "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                    ),
                    "parameters": { "type": "object", "properties": {} },
                })
            } else {
                json!({
                    "type": "function",
                    "name": encode_tool_name(&t.name),
                    "description": t.description,
                    "parameters": t.input_schema,
                })
            }
        })
        .collect()
}

// =============================================================================
// Streaming state + event parsing
// =============================================================================

/// Maximum accumulated size of a single tool call's streamed argument JSON.
/// Mirrors the chat path's `MAX_TOOL_ARGS_BYTES` guard so a runaway/malicious
/// Responses stream cannot grow the buffer without bound.
const MAX_TOOL_ARGS_BYTES: usize = 8 * 1024 * 1024;

/// In-flight accumulator for a single `function_call` output item.
#[derive(Default)]
struct ResponsesToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Streaming state for the Responses event loop. Tracks the currently-open
/// output item (so deltas route to the right block) and the deferred `Done`
/// event flushed when the terminal `response.completed` frame arrives.
pub(crate) struct ResponsesStreamState {
    /// The function-call item currently being assembled, if any.
    current_tool: Option<ResponsesToolCall>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    /// Whether any tool call was emitted this turn — used to map the
    /// completed-status stop reason to `ToolUse`.
    saw_tool_call: bool,
}

impl ResponsesStreamState {
    pub(crate) fn new() -> Self {
        Self {
            current_tool: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            saw_tool_call: false,
        }
    }
}

/// True when a raw Responses SSE `data:` frame is a stream-terminal *error*
/// frame (`response.failed` or a top-level `error`). The SSE loop uses this to
/// decide terminality from the frame type rather than the emitted `LlmEvent`
/// variant — a per-tool-call argument-parse error (emitted on an
/// `output_item.done` frame) is NOT terminal and must not suppress the
/// truncation guard.
pub(crate) fn is_terminal_error_frame(data: &str) -> bool {
    let Ok(json) = serde_json::from_str::<Value>(data) else {
        return false;
    };
    matches!(
        json.get("type").and_then(Value::as_str),
        Some("response.failed") | Some("error")
    )
}

/// Parse one Responses SSE `data:` frame into zero or more [`LlmEvent`]s.
///
/// Returns a terminal event (`Done` or `Error`) only for `response.completed`,
/// `response.done`, `response.incomplete`, `response.failed`, and `error`
/// frames — the caller treats those as the end of the stream and enforces
/// truncation-detection (error if the byte stream closes without one),
/// mirroring the chat path. `response.done` / `response.incomplete` are the
/// ChatGPT Codex backend's terminal-success / truncated frames.
pub(crate) fn parse_responses_event(data: &str, state: &mut ResponsesStreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            let preview: String = data.chars().take(200).collect();
            tracing::warn!(
                target: "wcore_providers::openai_responses",
                error = %e,
                payload = %preview,
                "discarding malformed OpenAI Responses SSE data frame"
            );
            return events;
        }
    };

    let Some(event_type) = json.get("type").and_then(Value::as_str) else {
        return events;
    };

    match event_type {
        // --- text -----------------------------------------------------------
        // `response.refusal.delta` carries a model refusal as text deltas; the
        // Codex backend emits it instead of `output_text.delta` for a refused
        // turn. Surface it as text so the refusal reaches the user instead of
        // streaming empty (additive — the plain-OpenAI path simply never emits
        // this frame).
        "response.output_text.delta" | "response.refusal.delta" => {
            if let Some(delta) = json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                events.push(LlmEvent::TextDelta(delta.to_string()));
            }
        }

        // --- reasoning / thinking ------------------------------------------
        // OpenAI emits reasoning as summary-text deltas (and, on some models,
        // raw reasoning-text deltas). Route both to ThinkingDelta.
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                events.push(LlmEvent::ThinkingDelta(delta.to_string()));
            }
        }

        // --- tool-call lifecycle -------------------------------------------
        "response.output_item.added" => {
            // A new output item starts. We only need to capture function_call
            // items (text/reasoning deltas carry their own content); seed the
            // accumulator from the item's id/name and any inline arguments.
            if let Some(item) = json.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
            {
                state.current_tool = Some(ResponsesToolCall {
                    call_id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }

        "response.function_call_arguments.delta" => {
            if let Some(tool) = state.current_tool.as_mut()
                && let Some(delta) = json.get("delta").and_then(Value::as_str)
            {
                if tool.arguments.len().saturating_add(delta.len()) > MAX_TOOL_ARGS_BYTES {
                    events.push(LlmEvent::Error(format!(
                        "tool-call arguments exceeded {MAX_TOOL_ARGS_BYTES} bytes — \
                         aborting stream to bound memory"
                    )));
                    return events;
                }
                tool.arguments.push_str(delta);
            }
        }

        "response.function_call_arguments.done" => {
            // OpenAI sends the authoritative complete argument string here.
            // Prefer it over the accumulated deltas when present.
            if let Some(tool) = state.current_tool.as_mut()
                && let Some(args) = json.get("arguments").and_then(Value::as_str)
                && !args.is_empty()
            {
                tool.arguments = args.to_string();
            }
        }

        "response.output_item.done" => {
            // Finalize a function_call item → emit ToolUse. (Text/reasoning
            // items finalize via their own done events and need no action;
            // their deltas were already streamed.)
            if let Some(item) = json.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(event) = finalize_tool_call(item, state)
            {
                events.push(event);
            }
        }

        // --- terminal: success ---------------------------------------------
        // `response.done` is the ChatGPT **Codex** backend's terminal-success
        // frame (OpenClaw normalizes both `response.completed` and
        // `response.done` to the same end-of-turn). It carries the same
        // `response` object (status + usage), so it routes through the identical
        // path. Additive: the plain-OpenAI Responses path never emits
        // `response.done`, so the existing `response.completed` behavior is
        // unchanged.
        "response.completed" | "response.done" => {
            let response = json.get("response");
            update_usage(state, response);
            let status = response
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str);
            let stop_reason = map_responses_status(status, state.saw_tool_call);
            let finish_reason = map_responses_finish(status);
            events.push(LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: state.cache_read_tokens,
                },
            });
        }

        // --- terminal: truncated -------------------------------------------
        // `response.incomplete` is a Codex terminal frame for a turn that ran
        // out of output budget. Treat it as a clean end-of-turn capped by
        // length (`StopReason::MaxTokens`), pulling usage from the same
        // `response` object. Additive: the plain-OpenAI path signals truncation
        // via `response.completed` with `status:"incomplete"` (handled above)
        // and never emits this frame.
        "response.incomplete" => {
            let response = json.get("response");
            update_usage(state, response);
            events.push(LlmEvent::Done {
                stop_reason: StopReason::MaxTokens,
                finish_reason: FinishReason::Length,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: state.cache_read_tokens,
                },
            });
        }

        // --- terminal: failure ---------------------------------------------
        "response.failed" => {
            let msg = json
                .get("response")
                .and_then(|r| r.get("error"))
                .map(format_responses_error)
                .unwrap_or_else(|| "OpenAI Responses stream failed (no error detail)".to_string());
            events.push(LlmEvent::Error(msg));
        }

        "error" => {
            let code = json.get("code").and_then(Value::as_str);
            let message = json
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let msg = match code {
                Some(c) => format!("Error Code {c}: {message}"),
                None => message.to_string(),
            };
            events.push(LlmEvent::Error(msg));
        }

        // Lifecycle frames we don't translate (response.created,
        // output_item.added for non-tool items, content_part.*,
        // reasoning_summary_part.*, etc.). No-op.
        _ => {}
    }

    events
}

/// Finalize the open function-call item into a `ToolUse` event. Fails closed
/// on malformed argument JSON — never runs the tool with empty/garbage input
/// (mirrors the chat path's tool-call finalization).
fn finalize_tool_call(item: &Value, state: &mut ResponsesStreamState) -> Option<LlmEvent> {
    // Prefer the in-flight accumulator (it carries streamed deltas); fall back
    // to the done-item's own fields if no item was ever opened.
    let tool = state.current_tool.take();
    let (call_id, name, arguments) = match tool {
        Some(t) => (t.call_id, t.name, t.arguments),
        None => (
            item.get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            item.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            item.get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    };

    let trimmed = arguments.trim();
    let input = if trimmed.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => v,
            Err(e) => {
                return Some(LlmEvent::Error(format!(
                    "tool-call arguments for '{name}' did not parse as JSON: {e}"
                )));
            }
        }
    };

    state.saw_tool_call = true;
    Some(LlmEvent::ToolUse {
        id: call_id,
        name: decode_tool_name(&name),
        input,
        extra: None,
    })
}

/// Pull token usage out of a `response.completed` payload. OpenAI's Responses
/// usage reports `input_tokens` INCLUSIVE of cached tokens, so subtract the
/// cached portion to get the non-cached input (mirrors OpenClaw).
fn update_usage(state: &mut ResponsesStreamState, response: Option<&Value>) {
    let Some(usage) = response.and_then(|r| r.get("usage")) else {
        return;
    };
    let cached = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    state.input_tokens = input.saturating_sub(cached);
    state.cache_read_tokens = cached;
    state.output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(state.output_tokens);
}

/// Map a `response.completed` status to a wayland-core [`StopReason`].
/// Mirrors `mapStopReason` in `openai-responses-shared.ts`, then upgrades a
/// clean `stop` to `ToolUse` when tool calls were emitted (so the agent loop
/// continues), matching the chat path.
fn map_responses_status(status: Option<&str>, saw_tool_call: bool) -> StopReason {
    // `incomplete` is the only non-end-turn outcome on a completed frame (it
    // means the output was truncated, e.g. by max_output_tokens). Every other
    // status — completed, in_progress, queued, and the defensive
    // failed/cancelled (which normally arrive via response.failed) — is treated
    // as a clean end-of-turn here.
    if status == Some("incomplete") {
        return StopReason::MaxTokens;
    }
    if saw_tool_call {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    }
}

/// Map a `response.completed` status to a protocol-level [`FinishReason`].
fn map_responses_finish(status: Option<&str>) -> FinishReason {
    match status {
        Some("completed") | None => FinishReason::Stop,
        Some("incomplete") => FinishReason::Length,
        _ => FinishReason::Error,
    }
}

/// Format a Responses error object (`{ code, message }`) into a string.
fn format_responses_error(error: &Value) -> String {
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no message");
    format!("{code}: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text { text: text.into() }])
    }

    // --- request body mapping --------------------------------------------

    #[test]
    fn build_body_maps_system_to_instructions_and_messages_to_input() {
        let request = LlmRequest {
            model: "gpt-5".into(),
            system: "You are helpful.".into(),
            messages: vec![user_msg("Hello")],
            max_tokens: 4096,
            ..Default::default()
        };
        let body = build_responses_body(&request, &ProviderCompat::default());

        assert_eq!(body["model"], json!("gpt-5"));
        // System prompt lives in `instructions`, NOT in input.
        assert_eq!(body["instructions"], json!("You are helpful."));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_output_tokens"], json!(4096));
        // No `messages` field on the Responses surface.
        assert!(body.get("messages").is_none());

        let input = body["input"].as_array().expect("input is an array");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["role"], json!("user"));
        assert_eq!(input[0]["content"][0]["type"], json!("input_text"));
        assert_eq!(input[0]["content"][0]["text"], json!("Hello"));
    }

    #[test]
    fn build_body_emits_flat_responses_tools_and_reasoning_effort() {
        let request = LlmRequest {
            model: "gpt-5".into(),
            system: String::new(),
            messages: vec![user_msg("hi")],
            max_tokens: 256,
            reasoning_effort: Some("high".into()),
            tools: vec![ToolDef {
                name: "read".into(),
                description: "Read a file".into(),
                input_schema: json!({ "type": "object", "properties": {} }),
                deferred: false,
            }],
            ..Default::default()
        };
        let body = build_responses_body(&request, &ProviderCompat::default());

        // No instructions when system is empty.
        assert!(body.get("instructions").is_none());
        // reasoning.effort threaded from the request effort.
        assert_eq!(body["reasoning"]["effort"], json!("high"));

        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        // FLAT shape: name/parameters at top level, NOT nested under "function".
        assert_eq!(tools[0]["type"], json!("function"));
        assert_eq!(tools[0]["name"], json!("read"));
        assert_eq!(tools[0]["description"], json!("Read a file"));
        assert!(tools[0]["parameters"].is_object());
        assert!(tools[0].get("function").is_none());
    }

    #[test]
    fn build_input_lowers_tool_call_and_result_round_trip() {
        // Assistant tool call + a following tool result lower to
        // function_call + function_call_output items keyed by call_id.
        let assistant = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_abc".into(),
                name: "read".into(),
                input: json!({ "path": "x.txt" }),
                extra: None,
            }],
        );
        let tool_result = Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_abc".into(),
                content: "file contents".into(),
                is_error: false,
            }],
        );

        let input = build_input(&[assistant, tool_result]);
        assert_eq!(input.len(), 2);

        assert_eq!(input[0]["type"], json!("function_call"));
        assert_eq!(input[0]["call_id"], json!("call_abc"));
        assert_eq!(input[0]["name"], json!("read"));
        // arguments is a serialized JSON STRING, not an object.
        assert_eq!(input[0]["arguments"], json!("{\"path\":\"x.txt\"}"));

        assert_eq!(input[1]["type"], json!("function_call_output"));
        assert_eq!(input[1]["call_id"], json!("call_abc"));
        assert_eq!(input[1]["output"], json!("file contents"));
    }

    #[test]
    fn build_input_drops_orphaned_function_call_output() {
        // A tool result whose parent assistant function_call was trimmed from
        // history is an orphan: the Responses API 400s on it. It must be
        // dropped, while a matched call/output pair is preserved.
        let orphan_result = Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_orphan".into(),
                content: "stale result".into(),
                is_error: false,
            }],
        );
        let assistant = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_live".into(),
                name: "read".into(),
                input: json!({ "path": "x.txt" }),
                extra: None,
            }],
        );
        let matched_result = Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_live".into(),
                content: "file contents".into(),
                is_error: false,
            }],
        );

        let input = build_input(&[orphan_result, assistant, matched_result]);

        // The orphaned function_call_output (call_orphan) is gone.
        assert!(
            !input.iter().any(|item| {
                item["type"] == json!("function_call_output")
                    && item["call_id"] == json!("call_orphan")
            }),
            "orphaned function_call_output must be dropped, got {input:?}"
        );
        // The matched function_call + function_call_output pair survives.
        assert!(input.iter().any(|item| {
            item["type"] == json!("function_call") && item["call_id"] == json!("call_live")
        }));
        assert!(input.iter().any(|item| {
            item["type"] == json!("function_call_output") && item["call_id"] == json!("call_live")
        }));
    }

    #[test]
    fn build_input_assistant_text_is_output_text() {
        let assistant = Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "the answer".into(),
            }],
        );
        let input = build_input(&[assistant]);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["role"], json!("assistant"));
        assert_eq!(input[0]["content"][0]["type"], json!("output_text"));
        assert_eq!(input[0]["content"][0]["text"], json!("the answer"));
    }

    // --- SSE event parsing ------------------------------------------------

    /// A realistic Responses stream: created → text item → text delta →
    /// function_call item → arg deltas → arg done → item done → completed.
    /// Asserts the exact LlmEvent sequence (text delta + tool use + done).
    #[test]
    fn parse_stream_text_then_tool_call_then_completed() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();

        let frames = [
            r#"{"type":"response.created","response":{"id":"resp_1"}}"#,
            r#"{"type":"response.output_item.added","item":{"type":"message","role":"assistant"}}"#,
            r#"{"type":"response.output_text.delta","delta":"Let me check."}"#,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_1","name":"read","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","delta":"{\"path\":"}"#,
            r#"{"type":"response.function_call_arguments.delta","delta":"\"a.txt\"}"}"#,
            r#"{"type":"response.function_call_arguments.done","arguments":"{\"path\":\"a.txt\"}"}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read"}}"#,
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120,"input_tokens_details":{"cached_tokens":10}}}}"#,
        ];
        for f in frames {
            got.extend(parse_responses_event(f, &mut state));
        }

        // Expect: one TextDelta, one ToolUse, one Done — in that order.
        assert_eq!(got.len(), 3, "events: {got:?}");

        match &got[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "Let me check."),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        match &got[1] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read");
                assert_eq!(input, &json!({ "path": "a.txt" }));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &got[2] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                // tool call present → ToolUse so the agent loop continues.
                assert_eq!(*stop_reason, StopReason::ToolUse);
                assert_eq!(*finish_reason, FinishReason::Stop);
                // cached subtracted from input: 100 - 10 = 90; cache_read = 10.
                assert_eq!(usage.input_tokens, 90);
                assert_eq!(usage.cache_read_tokens, 10);
                assert_eq!(usage.output_tokens, 20);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_completed_without_tool_calls_is_end_turn() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        got.extend(parse_responses_event(
            r#"{"type":"response.output_text.delta","delta":"hello"}"#,
            &mut state,
        ));
        got.extend(parse_responses_event(
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":5,"output_tokens":2}}}"#,
            &mut state,
        ));
        assert!(matches!(got[0], LlmEvent::TextDelta(_)));
        match &got[1] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(*finish_reason, FinishReason::Stop);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_reasoning_summary_delta_is_thinking() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"response.reasoning_summary_text.delta","delta":"weighing options"}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "weighing options"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_malformed_tool_args_fails_closed() {
        // arguments that never parse must NOT run the tool with empty input —
        // emit an Error and skip the call.
        let mut state = ResponsesStreamState::new();
        let _ = parse_responses_event(
            r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"bad","arguments":""}}"#,
            &mut state,
        );
        let _ = parse_responses_event(
            r#"{"type":"response.function_call_arguments.delta","delta":"{not json"}"#,
            &mut state,
        );
        let events = parse_responses_event(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"c1","name":"bad"}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => assert!(msg.contains("did not parse as JSON")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_failed_is_error() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"boom"}}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => {
                assert!(msg.contains("server_error"));
                assert!(msg.contains("boom"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_frame_is_error() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"error","code":"rate_limit","message":"slow down"}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => {
                assert!(msg.contains("rate_limit"));
                assert!(msg.contains("slow down"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_incomplete_status_maps_to_max_tokens() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"response.completed","response":{"status":"incomplete","usage":{"input_tokens":1,"output_tokens":1}}}"#,
            &mut state,
        );
        match &events[0] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::MaxTokens);
                assert_eq!(*finish_reason, FinishReason::Length);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_malformed_frame_is_discarded() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event("not json", &mut state);
        assert!(events.is_empty());
    }

    // --- Codex terminal-frame aliases (D1) -------------------------------

    /// `response.done` is the Codex terminal-success frame and must produce a
    /// `Done` exactly like `response.completed` (same status/usage handling).
    #[test]
    fn parse_response_done_is_terminal_like_completed() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        got.extend(parse_responses_event(
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
            &mut state,
        ));
        got.extend(parse_responses_event(
            r#"{"type":"response.done","response":{"status":"completed","usage":{"input_tokens":7,"output_tokens":3,"input_tokens_details":{"cached_tokens":2}}}}"#,
            &mut state,
        ));
        assert!(matches!(got[0], LlmEvent::TextDelta(_)));
        match &got[1] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(*finish_reason, FinishReason::Stop);
                // cached subtracted: 7 - 2 = 5; cache_read = 2.
                assert_eq!(usage.input_tokens, 5);
                assert_eq!(usage.cache_read_tokens, 2);
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// `response.done` after a tool call still upgrades the stop reason to
    /// `ToolUse` so the agent loop continues (parity with `response.completed`).
    #[test]
    fn parse_response_done_after_tool_call_is_tool_use() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"read","arguments":"{}"}}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"c1","name":"read"}}"#,
            r#"{"type":"response.done","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert!(matches!(got[0], LlmEvent::ToolUse { .. }));
        match &got[1] {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// `response.incomplete` is a Codex terminal frame for a length-truncated
    /// turn → `Done` with `StopReason::MaxTokens` / `FinishReason::Length`.
    #[test]
    fn parse_response_incomplete_is_max_tokens_done() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"response.incomplete","response":{"status":"incomplete","usage":{"input_tokens":9,"output_tokens":4}}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                assert_eq!(*stop_reason, StopReason::MaxTokens);
                assert_eq!(*finish_reason, FinishReason::Length);
                assert_eq!(usage.input_tokens, 9);
                assert_eq!(usage.output_tokens, 4);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// A Codex refusal streams via `response.refusal.delta` and must surface as
    /// text (not an empty turn).
    #[test]
    fn parse_refusal_delta_is_text() {
        let mut state = ResponsesStreamState::new();
        let events = parse_responses_event(
            r#"{"type":"response.refusal.delta","delta":"I can't help with that."}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "I can't help with that."),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }
}
