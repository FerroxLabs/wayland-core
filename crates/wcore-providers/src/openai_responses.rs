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

use crate::tool_name::{decode_tool_name, encode_tool_name};
use wcore_config::compat::ProviderCompat;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::tool::{ToolDef, truncate_deferred_description};

/// Build the `/v1/responses` request body from a provider-neutral
/// [`LlmRequest`]. Mirrors `buildParams` + `convertResponsesMessages` +
/// `convertResponsesTools` from the OpenClaw references in the module docs.
pub(crate) fn build_responses_body(request: &LlmRequest, compat: &ProviderCompat) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": build_input(&request.messages),
        "stream": true,
        // `store: false` keeps OpenAI from persisting the response server-side;
        // wayland-core manages its own history. Mirrors OpenClaw's default.
        "store": false,
    });

    // #112: when the engine flagged this turn omit-safe (user omitted the cap
    // + model unknown to the registry + provider tolerates the absent field),
    // skip `max_output_tokens` so the served model's natural ceiling applies.
    // Belt-and-braces: also gated on THIS provider's own compat so a request
    // built against another provider's compat can never strip the field.
    if !(request.omit_max_tokens && compat.omit_max_tokens_when_unsized()) {
        body["max_output_tokens"] = json!(request.max_tokens);
    }

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

    // Empty call_ids are excluded outright: a `ToolUse { id: "" }` persisted
    // by a pre-#133 session would otherwise round-trip a `call_id: ""` pair
    // that strict endpoints 400 on (parity with the Chat path's empty-id
    // strip in `openai.rs`).
    let called_ids: HashSet<String> = input
        .iter()
        .filter(|item| item["type"].as_str() == Some("function_call"))
        .filter_map(|item| item["call_id"].as_str().map(String::from))
        .filter(|id| !id.is_empty())
        .collect();

    input.retain(|item| match item["type"].as_str() {
        // Drop empty-id calls (see above); keep the rest.
        Some("function_call") => item["call_id"].as_str().is_some_and(|id| !id.is_empty()),
        // Keep only outputs whose call survives in the input set. An output
        // whose call_id is unmatched — OR missing/non-string/empty — is an
        // orphan the Responses API 400s on, so drop it
        // (FerroxLabs/wayland-core#123). Unlike the Chat path there is no
        // separate empty-id guard here, so this pass must be self-sufficient.
        Some("function_call_output") => item["call_id"]
            .as_str()
            .is_some_and(|id| called_ids.contains(id)),
        // Non-call items are out of scope for this pass.
        _ => true,
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

    // Inline images become `input_image` parts alongside any `input_text`.
    // Native Responses shape: `{type:input_image, image_url:"data:<mime>;base64,<b64>"}`.
    let mut content: Vec<Value> = Vec::new();
    let text: String = collect_text(msg);
    if !text.is_empty() {
        content.push(json!({ "type": "input_text", "text": text }));
    }
    for block in &msg.content {
        if let ContentBlock::Image { mime, data } = block {
            content.push(json!({
                "type": "input_image",
                "image_url": format!("data:{mime};base64,{data}"),
            }));
        }
    }
    if content.is_empty() {
        return;
    }
    input.push(json!({
        "type": "message",
        "role": "user",
        "content": content,
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
                    // Layer D2: no per-stub "use ToolSearch" boilerplate —
                    // the system prompt states the hydration rule once.
                    "description": format!("(Deferred) {short_desc}"),
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

/// Maximum number of concurrently-open `function_call` items. The argument
/// guard above is per-entry, so without this cap a malicious stream could
/// open unbounded items at up to `MAX_TOOL_ARGS_BYTES` each. Real turns carry
/// a handful of parallel calls; 128 is far past any legitimate stream.
const MAX_OPEN_TOOL_CALLS: usize = 128;

/// Terminal error for a tool call whose argument payload breaches
/// [`MAX_TOOL_ARGS_BYTES`] (streamed, inline-on-added, or arguments.done).
fn args_overflow_error() -> LlmEvent {
    LlmEvent::Error(format!(
        "tool-call arguments exceeded {MAX_TOOL_ARGS_BYTES} bytes — \
         aborting stream to bound memory"
    ))
}

/// In-flight accumulator for a single `function_call` output item.
#[derive(Default)]
struct ResponsesToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Streaming state for the Responses event loop. Tracks the currently-open
/// output items (so deltas route to the right block) and the deferred `Done`
/// event flushed when the terminal `response.completed` frame arrives.
pub(crate) struct ResponsesStreamState {
    /// Function-call items currently being assembled, keyed by
    /// [`tool_item_key`]. A map (not a single slot) so PARALLEL tool calls
    /// whose `output_item.added` frames interleave never cross-wire each
    /// other's `call_id`/arguments (#133 — desktop stuck-spinner root cause).
    open_tools: std::collections::HashMap<String, ResponsesToolCall>,
    /// Alternate-key → primary-key aliases (`output_index:N` → item id),
    /// recorded on `output_item.added` so argument frames that carry only the
    /// output_index still resolve to an item-id-keyed entry (and vice versa
    /// via the lookup's index-tier fallback). Purged when the entry closes.
    key_aliases: std::collections::HashMap<String, String>,
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
            open_tools: std::collections::HashMap::new(),
            key_aliases: std::collections::HashMap::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            saw_tool_call: false,
        }
    }
}

/// Correlation key tying a `function_call` item's `output_item.added`,
/// `function_call_arguments.*`, and `output_item.done` frames together.
///
/// Prefers the server-assigned item id (`item.id` on added/done frames,
/// `item_id` on argument frames — the `fc_...` identifier, present on both
/// the plain-OpenAI and ChatGPT Codex backends even when `call_id` is absent
/// from the `added` frame). Falls back to the frame's `output_index`, then to
/// a fixed sentinel so a stream carrying neither still routes argument deltas
/// to the single open tool (pre-#133 behaviour).
fn tool_item_key(frame: &Value) -> String {
    let item_id = frame
        .get("item")
        .and_then(|i| i.get("id"))
        .and_then(Value::as_str)
        .or_else(|| frame.get("item_id").and_then(Value::as_str));
    if let Some(id) = item_id
        && !id.is_empty()
    {
        return id.to_string();
    }
    match frame.get("output_index").and_then(Value::as_u64) {
        Some(idx) => format!("output_index:{idx}"),
        None => "single".to_string(),
    }
}

/// Resolve the `open_tools` key an argument/done frame refers to, tolerating
/// mixed-tier streams where one frame carries the item id and another only
/// the `output_index`. Tries, in order: the frame's primary key, its alias,
/// the index-tier key, and that key's alias. Returns `None` when no open
/// entry matches on any tier.
fn resolve_open_tool_key(state: &ResponsesStreamState, frame: &Value) -> Option<String> {
    let resolve = |key: String| -> Option<String> {
        if state.open_tools.contains_key(&key) {
            return Some(key);
        }
        state
            .key_aliases
            .get(&key)
            .filter(|target| state.open_tools.contains_key(*target))
            .cloned()
    };
    let primary = tool_item_key(frame);
    if let Some(key) = resolve(primary) {
        return Some(key);
    }
    frame
        .get("output_index")
        .and_then(Value::as_u64)
        .and_then(|idx| resolve(format!("output_index:{idx}")))
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
                let key = tool_item_key(&json);
                // Bound the map: MAX_TOOL_ARGS_BYTES is per-entry, so entry
                // count must be capped too or total memory is unbounded.
                if !state.open_tools.contains_key(&key)
                    && state.open_tools.len() >= MAX_OPEN_TOOL_CALLS
                {
                    events.push(LlmEvent::Error(format!(
                        "more than {MAX_OPEN_TOOL_CALLS} concurrent tool-call items — \
                         aborting stream to bound memory"
                    )));
                    return events;
                }
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // Inline arguments on the added frame count against the same
                // byte bound as streamed deltas.
                if arguments.len() > MAX_TOOL_ARGS_BYTES {
                    events.push(args_overflow_error());
                    return events;
                }
                // Mixed-tier alias: later argument frames may carry only the
                // output_index while this frame carried the item id (or vice
                // versa) — record index→primary so lookups resolve on either
                // tier instead of silently dropping deltas.
                if let Some(idx) = json.get("output_index").and_then(Value::as_u64) {
                    let idx_key = format!("output_index:{idx}");
                    if idx_key != key {
                        state.key_aliases.insert(idx_key, key.clone());
                    }
                }
                state.open_tools.insert(
                    key,
                    ResponsesToolCall {
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
                        arguments: arguments.to_string(),
                    },
                );
            }
        }

        "response.function_call_arguments.delta" => {
            if let Some(delta) = json.get("delta").and_then(Value::as_str) {
                match resolve_open_tool_key(state, &json) {
                    Some(key) => {
                        if let Some(tool) = state.open_tools.get_mut(&key) {
                            if tool.arguments.len().saturating_add(delta.len())
                                > MAX_TOOL_ARGS_BYTES
                            {
                                events.push(args_overflow_error());
                                return events;
                            }
                            tool.arguments.push_str(delta);
                        }
                    }
                    // A silent drop here runs the tool with `{}` when the done
                    // item also omits inline arguments — make it loud.
                    None => tracing::warn!(
                        target: "wcore_providers::openai_responses",
                        "dropping function_call_arguments.delta with no open \
                         function_call item"
                    ),
                }
            }
        }

        "response.function_call_arguments.done" => {
            // OpenAI sends the authoritative complete argument string here.
            // Prefer it over the accumulated deltas when present.
            if let Some(args) = json.get("arguments").and_then(Value::as_str)
                && !args.is_empty()
            {
                if args.len() > MAX_TOOL_ARGS_BYTES {
                    events.push(args_overflow_error());
                    return events;
                }
                match resolve_open_tool_key(state, &json) {
                    Some(key) => {
                        if let Some(tool) = state.open_tools.get_mut(&key) {
                            tool.arguments = args.to_string();
                        }
                    }
                    None => tracing::warn!(
                        target: "wcore_providers::openai_responses",
                        "dropping function_call_arguments.done with no open \
                         function_call item"
                    ),
                }
            }
        }

        "response.output_item.done" => {
            // Finalize a function_call item → emit ToolUse. (Text/reasoning
            // items finalize via their own done events and need no action;
            // their deltas were already streamed.)
            if let Some(item) = json.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(event) = finalize_tool_call(&json, item, state)
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
///
/// #133 call_id stability: the emitted `ToolUse.id` seeds the json-stream
/// protocol's `tool_request`/`tool_running`/`tool_result` `call_id`, which
/// hosts (the Wayland desktop) merge tool cards by. Field merging prefers the
/// done-item's identity fields (the Codex backend can omit `call_id` from the
/// `output_item.added` frame and only supply it here), falls back to the
/// accumulator, and finally to the item's server id — the id is NEVER empty.
fn finalize_tool_call(
    frame: &Value,
    item: &Value,
    state: &mut ResponsesStreamState,
) -> Option<LlmEvent> {
    let key = resolve_open_tool_key(state, frame).unwrap_or_else(|| tool_item_key(frame));
    let acc = state.open_tools.remove(&key).unwrap_or_default();
    state.key_aliases.retain(|_, target| target != &key);

    let item_str = |field: &str| -> Option<&str> {
        item.get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let non_empty = |s: String| -> Option<String> { (!s.is_empty()).then_some(s) };

    // Identity: the done item is authoritative; the accumulator (seeded from
    // the `added` frame) is the fallback. Never emit an empty id — an empty
    // `call_id` makes parallel tool cards collide host-side and produces a
    // `function_call_output` the API rejects. Last resort is the item's
    // server-assigned id / correlation key, which is stable across all three
    // protocol frames because the engine threads `ToolUse.id` verbatim.
    let call_id = item_str("call_id")
        .map(str::to_string)
        .or_else(|| non_empty(acc.call_id))
        .or_else(|| item_str("id").map(str::to_string))
        .unwrap_or_else(|| {
            tracing::warn!(
                target: "wcore_providers::openai_responses",
                key = %key,
                "function_call item carried no call_id or id on any frame; \
                 falling back to the correlation key"
            );
            key
        });
    let name = item_str("name")
        .map(str::to_string)
        .or(non_empty(acc.name))
        .unwrap_or_default();
    // Arguments: the accumulator carries the streamed deltas (and the
    // authoritative `function_call_arguments.done` string when one arrived);
    // the done-item's inline `arguments` is the fallback.
    let arguments = non_empty(acc.arguments)
        .or_else(|| item_str("arguments").map(str::to_string))
        .unwrap_or_default();

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
    fn build_input_user_image_becomes_input_image() {
        let msg = Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "what is this".into(),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "QUJD".into(),
                },
            ],
        );
        let input = build_input(&[msg]);
        assert_eq!(input.len(), 1);
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "what is this");
        // Responses native image-input shape.
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,QUJD");
    }

    #[test]
    fn build_input_image_only_user_turn_is_not_dropped() {
        // A turn that is only an image (no text) must still produce an input item.
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                mime: "image/jpeg".into(),
                data: "QUJD".into(),
            }],
        );
        let input = build_input(&[msg]);
        assert_eq!(input.len(), 1);
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "input_image");
    }

    /// #112: when the engine flags `omit_max_tokens` AND the provider's own
    /// compat is omit-safe, the Responses body carries NO `max_output_tokens`
    /// — the served model's ceiling applies.
    #[test]
    fn build_body_omits_max_output_tokens_when_flagged() {
        let request = LlmRequest {
            model: "gpt-5-unlisted".into(),
            system: String::new(),
            messages: vec![user_msg("hi")],
            max_tokens: 8_192, // sized internal budget stays positive
            omit_max_tokens: true,
            ..Default::default()
        };
        let omit_safe = ProviderCompat {
            omit_max_tokens_when_unsized: Some(true),
            ..ProviderCompat::default()
        };
        let body = build_responses_body(&request, &omit_safe);
        assert!(
            body.get("max_output_tokens").is_none(),
            "omit_max_tokens must drop max_output_tokens from the wire body"
        );

        // #112 belt-and-braces: with a compat that is NOT omit-safe, the
        // request flag alone must not strip the field.
        let body = build_responses_body(&request, &ProviderCompat::default());
        assert_eq!(
            body["max_output_tokens"],
            json!(8_192),
            "a non-omit-safe compat must keep sending the sized value"
        );
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
                server: None,
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

    /// #139 (Responses path): plugin fq_names must be wire-legal in the FLAT
    /// tool definitions AND in replayed history `function_call` items — same
    /// contract as the chat path, since strict backends enforce
    /// `^[a-zA-Z0-9_-]+$` on both.
    #[test]
    fn build_body_encodes_plugin_fq_tool_names_issue_139() {
        let raw = "Browser::execute";
        let request = LlmRequest {
            model: "gpt-5".into(),
            system: String::new(),
            messages: vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: raw.into(),
                        input: json!({"cmd": "ls"}),
                        extra: None,
                    }],
                ),
                Message::new(
                    Role::Tool,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "ok".into(),
                        is_error: false,
                    }],
                ),
            ],
            max_tokens: 256,
            tools: vec![
                ToolDef {
                    name: raw.into(),
                    description: "run a browser action".into(),
                    input_schema: json!({ "type": "object", "properties": {} }),
                    deferred: false,
                    server: None,
                },
                ToolDef {
                    name: "get_weather".into(),
                    description: "w".into(),
                    input_schema: json!({ "type": "object", "properties": {} }),
                    deferred: false,
                    server: None,
                },
            ],
            ..Default::default()
        };
        let body = build_responses_body(&request, &ProviderCompat::default());
        let wire_legal = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };

        // FLAT tool defs: every name legal; the dirty one decodes back.
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2);
        for t in tools {
            let name = t["name"].as_str().unwrap();
            assert!(
                wire_legal(name),
                "illegal tool-def name on the wire: {name}"
            );
        }
        let wire_name = tools[0]["name"].as_str().unwrap();
        assert_ne!(wire_name, raw, "fq_name must not leak raw");
        assert_eq!(decode_tool_name(wire_name), raw);
        assert_eq!(tools[1]["name"], json!("get_weather"));

        // History function_call items carry the SAME encoded spelling.
        let input = body["input"].as_array().expect("input array");
        let call = input
            .iter()
            .find(|i| i["type"] == "function_call")
            .expect("function_call item");
        assert_eq!(call["name"].as_str().unwrap(), wire_name);
    }

    /// #139 inbound half on the Responses path: a `function_call` streamed
    /// under the SANITIZED wire name must surface as `LlmEvent::ToolUse` with
    /// the ORIGINAL registry fq_name for dispatch.
    #[test]
    fn parse_sanitized_tool_call_dispatches_original_name_issue_139() {
        let raw = "Browser::execute";
        let wire = encode_tool_name(raw);
        assert_ne!(wire, raw, "fixture must actually be encoded");

        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        let frames = [
            format!(
                r#"{{"type":"response.output_item.added","item":{{"type":"function_call","call_id":"call_1","name":"{wire}","arguments":""}}}}"#
            ),
            r#"{"type":"response.function_call_arguments.delta","delta":"{\"cmd\":\"ls\"}"}"#
                .to_string(),
            format!(
                r#"{{"type":"response.output_item.done","item":{{"type":"function_call","call_id":"call_1","name":"{wire}"}}}}"#
            ),
        ];
        for f in &frames {
            got.extend(parse_responses_event(f, &mut state));
        }

        let (name, input) = got
            .iter()
            .find_map(|e| match e {
                LlmEvent::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
                _ => None,
            })
            .expect("ToolUse event");
        assert_eq!(
            name, raw,
            "inbound function_call must decode to the canonical fq_name"
        );
        assert_eq!(input, json!({"cmd": "ls"}));
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

    /// #123: `clean_orphaned_function_call_outputs` must drop a
    /// `function_call_output` whose `call_id` is missing/non-string, not just
    /// the present-but-unmatched case. The Responses path has no separate
    /// empty-id guard, so this pass must be self-sufficient.
    #[test]
    fn clean_orphaned_outputs_strips_missing_call_id() {
        let mut input = vec![
            json!({ "type": "function_call", "call_id": "ok",
                "name": "read", "arguments": "{}" }),
            json!({ "type": "function_call_output", "call_id": "ok", "output": "kept" }),
            // Unmatched id.
            json!({ "type": "function_call_output", "call_id": "gone", "output": "x" }),
            // Missing call_id entirely (previously "out of scope").
            json!({ "type": "function_call_output", "output": "no id" }),
        ];

        clean_orphaned_function_call_outputs(&mut input);

        let outputs: Vec<_> = input
            .iter()
            .filter(|i| i["type"] == json!("function_call_output"))
            .collect();
        assert_eq!(outputs.len(), 1, "only the matched output survives");
        assert_eq!(outputs[0]["call_id"], json!("ok"));
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

    // --- #133 call_id stability -------------------------------------------

    /// Codex-shaped stream: the `output_item.added` frame carries the item id
    /// (`fc_...`) but NO `call_id`; only the `output_item.done` frame carries
    /// the real `call_id`. The emitted `ToolUse.id` must be the real call_id —
    /// never the empty string the added-frame accumulator was seeded with
    /// (pre-fix, the accumulator won wholesale and the id streamed as "").
    #[test]
    fn parse_added_without_call_id_uses_done_frame_call_id() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","name":"read","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":\"a.txt\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_real","name":"read"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 1, "events: {got:?}");
        match &got[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_real", "id must be the done-frame call_id");
                assert!(!id.is_empty(), "ToolUse id must never be empty");
                assert_eq!(name, "read");
                assert_eq!(input, &json!({ "path": "a.txt" }));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// Two PARALLEL function_call items streamed sequentially (added/deltas/
    /// done per item) must emit two ToolUse events with the correct distinct
    /// ids and each item's own arguments.
    #[test]
    fn parse_parallel_sequential_tool_calls_keep_distinct_ids() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":"{\"path\":\"a\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}}"#,
            r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"grep","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_b","output_index":1,"delta":"{\"pattern\":\"b\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"grep"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 2, "events: {got:?}");
        match (&got[0], &got[1]) {
            (
                LlmEvent::ToolUse {
                    id: id_a,
                    input: in_a,
                    ..
                },
                LlmEvent::ToolUse {
                    id: id_b,
                    input: in_b,
                    ..
                },
            ) => {
                assert_eq!(id_a, "call_a");
                assert_eq!(in_a, &json!({ "path": "a" }));
                assert_eq!(id_b, "call_b");
                assert_eq!(in_b, &json!({ "pattern": "b" }));
            }
            other => panic!("expected two ToolUse events, got {other:?}"),
        }
    }

    /// Two parallel function_call items whose frames INTERLEAVE (added A,
    /// added B, delta A, delta B, done A, done B). Pre-fix, the single
    /// `current_tool` slot let B's `added` overwrite in-flight A: A finalized
    /// with B's call_id (duplicate id, wrong args) and A's own call_id never
    /// reached the host — the #133 stuck-spinner shape. The per-item map must
    /// keep both calls fully separate.
    #[test]
    fn parse_parallel_interleaved_tool_calls_do_not_cross_wire() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}}"#,
            r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"grep","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":"{\"path\":\"a\"}"}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_b","output_index":1,"delta":"{\"pattern\":\"b\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}}"#,
            r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"grep"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 2, "events: {got:?}");
        let ids: Vec<&str> = got
            .iter()
            .map(|e| match e {
                LlmEvent::ToolUse { id, .. } => id.as_str(),
                other => panic!("expected ToolUse, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, vec!["call_a", "call_b"], "no duplicate/lost call_ids");
        match (&got[0], &got[1]) {
            (
                LlmEvent::ToolUse {
                    name: n_a,
                    input: in_a,
                    ..
                },
                LlmEvent::ToolUse {
                    name: n_b,
                    input: in_b,
                    ..
                },
            ) => {
                assert_eq!((n_a.as_str(), in_a), ("read", &json!({ "path": "a" })));
                assert_eq!((n_b.as_str(), in_b), ("grep", &json!({ "pattern": "b" })));
            }
            _ => unreachable!(),
        }
    }

    /// Degenerate stream with NO call_id on any frame: the ToolUse id falls
    /// back to the item's server id (`fc_...`) — never the empty string, so
    /// the protocol's three tool frames still agree and cards still merge.
    #[test]
    fn parse_tool_call_without_any_call_id_falls_back_to_item_id() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_9","name":"read","arguments":"{}"}}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_9","name":"read"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 1, "events: {got:?}");
        match &got[0] {
            LlmEvent::ToolUse { id, .. } => assert_eq!(id, "fc_9"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// A done frame whose item was never opened (no `added` frame seen) still
    /// finalizes entirely from the done-item's own fields.
    #[test]
    fn parse_done_without_added_finalizes_from_item_fields() {
        let mut state = ResponsesStreamState::new();
        let got = parse_responses_event(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_2","call_id":"call_2","name":"read","arguments":"{\"path\":\"z\"}"}}"#,
            &mut state,
        );
        assert_eq!(got.len(), 1, "events: {got:?}");
        match &got[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_2");
                assert_eq!(name, "read");
                assert_eq!(input, &json!({ "path": "z" }));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// Mixed-tier alias, direction 1: the added frame carries the item id,
    /// argument frames carry ONLY the output_index. The deltas must still
    /// reach the item-id-keyed entry (a silent drop would run the tool with
    /// `{}` when the done item omits inline arguments).
    #[test]
    fn parse_delta_with_only_output_index_reaches_item_id_keyed_tool() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":\"a\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"read"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 1, "events: {got:?}");
        match &got[0] {
            LlmEvent::ToolUse { id, input, .. } => {
                assert_eq!(id, "c1");
                assert_eq!(input, &json!({ "path": "a" }), "deltas must not drop");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// Mixed-tier alias, direction 2: the added frame carries ONLY the
    /// output_index, argument frames carry the item id (plus index). The
    /// lookup's index-tier fallback must resolve the entry.
    #[test]
    fn parse_delta_with_item_id_reaches_output_index_keyed_tool() {
        let mut state = ResponsesStreamState::new();
        let mut got: Vec<LlmEvent> = Vec::new();
        for f in [
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"c1","name":"read","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":\"b\"}"}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"read"}}"#,
        ] {
            got.extend(parse_responses_event(f, &mut state));
        }
        assert_eq!(got.len(), 1, "events: {got:?}");
        match &got[0] {
            LlmEvent::ToolUse { id, input, .. } => {
                assert_eq!(id, "c1");
                assert_eq!(input, &json!({ "path": "b" }), "deltas must not drop");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// The open-tools map is bounded: the added frame past
    /// `MAX_OPEN_TOOL_CALLS` entries aborts the stream with an error instead
    /// of growing memory without bound (each entry can hold up to
    /// MAX_TOOL_ARGS_BYTES of arguments).
    #[test]
    fn parse_open_tool_cap_errors_past_limit() {
        let mut state = ResponsesStreamState::new();
        for i in 0..MAX_OPEN_TOOL_CALLS {
            let frame = format!(
                r#"{{"type":"response.output_item.added","output_index":{i},"item":{{"type":"function_call","id":"fc_{i}","call_id":"c_{i}","name":"read","arguments":""}}}}"#
            );
            assert!(
                parse_responses_event(&frame, &mut state).is_empty(),
                "added frame {i} must be accepted silently"
            );
        }
        let over = format!(
            r#"{{"type":"response.output_item.added","output_index":{i},"item":{{"type":"function_call","id":"fc_{i}","call_id":"c_{i}","name":"read","arguments":""}}}}"#,
            i = MAX_OPEN_TOOL_CALLS
        );
        let events = parse_responses_event(&over, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => assert!(msg.contains("concurrent tool-call items")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// Inline `arguments` on the added frame count against the same byte
    /// bound as streamed deltas (previously bypassed the guard).
    #[test]
    fn parse_added_inline_arguments_over_byte_cap_errors() {
        let mut state = ResponsesStreamState::new();
        let big = "x".repeat(MAX_TOOL_ARGS_BYTES + 1);
        let frame = json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_1", "call_id": "c1", "name": "read",
                "arguments": big,
            },
        })
        .to_string();
        let events = parse_responses_event(&frame, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => assert!(msg.contains("exceeded")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// The authoritative `function_call_arguments.done` replacement string is
    /// also byte-bounded (previously bypassed the guard).
    #[test]
    fn parse_arguments_done_over_byte_cap_errors() {
        let mut state = ResponsesStreamState::new();
        let _ = parse_responses_event(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":""}}"#,
            &mut state,
        );
        let big = "x".repeat(MAX_TOOL_ARGS_BYTES + 1);
        let frame = json!({
            "type": "response.function_call_arguments.done",
            "item_id": "fc_1",
            "output_index": 0,
            "arguments": big,
        })
        .to_string();
        let events = parse_responses_event(&frame, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Error(msg) => assert!(msg.contains("exceeded")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A `call_id: ""` pair persisted by a pre-#133 session must not
    /// round-trip to the API (strict endpoints 400 on it): both the empty-id
    /// `function_call` and its empty-id output are stripped; valid pairs
    /// survive.
    #[test]
    fn clean_orphaned_outputs_strips_empty_call_id_pairs() {
        let mut input = vec![
            json!({"type":"function_call","call_id":"","name":"read","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"","output":"x"}),
            json!({"type":"function_call","call_id":"ok","name":"read","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"ok","output":"kept"}),
        ];
        clean_orphaned_function_call_outputs(&mut input);
        assert_eq!(input.len(), 2, "only the valid pair survives: {input:?}");
        assert!(input.iter().all(|i| i["call_id"] == json!("ok")));
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
