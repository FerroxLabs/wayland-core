// Shared Anthropic message/tool building and SSE parsing logic.
// Used by AnthropicProvider, BedrockProvider, and VertexProvider.

use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::tool::{ToolDef, truncate_deferred_description};

use super::ProviderError;
use crate::dump_response_chunk;
use crate::tool_name::{decode_tool_name, encode_tool_name};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

/// Convert internal Message format to Anthropic API message format.
/// Compat flags control merging and alternation behavior.
pub fn build_messages(messages: &[Message], compat: &ProviderCompat) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        let role_str = match msg.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue, // system is top-level in Anthropic
        };

        let mut content: Vec<Value> = msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(json!({
                    "type": "text",
                    "text": text
                })),
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    let tool_id = if id.is_empty() && compat.auto_tool_id() {
                        generate_tool_id()
                    } else {
                        id.clone()
                    };
                    Some(json!({
                        "type": "tool_use",
                        "id": tool_id,
                        "name": encode_tool_name(name),
                        "input": input
                    }))
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                    "is_error": is_error
                })),
                // wayland#161: a `thinking` block replayed in message history
                // must carry a valid `signature` — which we never capture, and
                // which a model switch would invalidate anyway. Anthropic 400s
                // on an unsigned thinking block
                // (`messages[n].content[m].thinking.signature`), stranding the
                // whole conversation as unrecoverable. Omitting thinking on
                // replay is always accepted, so drop it.
                ContentBlock::Thinking { .. } => None,
            })
            .collect();

        // Strip patterns from text content
        if let Some(patterns) = &compat.strip_patterns {
            for item in &mut content {
                if item["type"] == "text"
                    && let Some(text) = item["text"].as_str()
                {
                    let mut cleaned = text.to_string();
                    for pattern in patterns {
                        cleaned = cleaned.replace(pattern, "");
                    }
                    item["text"] = json!(cleaned);
                }
            }
        }

        // wayland#161: an assistant turn truncated mid-thinking (or one that
        // held only a thinking block) is now empty after the drop above.
        // Anthropic rejects a message with empty `content`, so skip the turn
        // rather than emit `content: []` and 400 the request.
        if content.is_empty() {
            continue;
        }

        // W1 Task 4: translate MessageCacheHint::Breakpoint into Anthropic
        // cache_control on the LAST content block. Bound to compat to avoid
        // emitting cache_control for downstream providers that don't honour it.
        if compat.cache_message_breakpoints()
            && matches!(
                msg.cache_breakpoint,
                Some(wcore_types::message::MessageCacheHint::Breakpoint)
            )
            && let Some(last_block) = content.last_mut()
            && let Some(obj) = last_block.as_object_mut()
        {
            obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        }

        // Merge consecutive messages with the same role (if enabled)
        if compat.merge_same_role()
            && let Some(last) = result.last_mut()
            && last["role"].as_str() == Some(role_str)
            && let Some(arr) = last["content"].as_array_mut()
        {
            arr.extend(content);
            continue;
        }

        result.push(json!({
            "role": role_str,
            "content": content
        }));
    }

    // Ensure user/assistant alternation (if enabled)
    if compat.ensure_alternation() {
        ensure_message_alternation(&mut result);
    }

    result
}

/// Insert filler messages to ensure strict user/assistant alternation.
fn ensure_message_alternation(messages: &mut Vec<Value>) {
    if messages.is_empty() {
        return;
    }

    // If first message is assistant, prepend a user filler
    if messages[0]["role"].as_str() == Some("assistant") {
        messages.insert(
            0,
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "."}]
            }),
        );
    }

    // Walk through and insert fillers where alternation is broken
    let mut i = 1;
    while i < messages.len() {
        let prev_role = messages[i - 1]["role"].as_str().unwrap_or("");
        let curr_role = messages[i]["role"].as_str().unwrap_or("");
        if prev_role == curr_role {
            let filler_role = if curr_role == "user" {
                "assistant"
            } else {
                "user"
            };
            messages.insert(
                i,
                json!({
                    "role": filler_role,
                    "content": [{"type": "text", "text": "."}]
                }),
            );
            i += 1; // skip the filler we just inserted
        }
        i += 1;
    }
}

/// Generate a unique tool ID when missing
fn generate_tool_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u32 = (ts as u32).wrapping_mul(2654435761); // simple hash
    format!("toolu_{:x}_{:08x}", ts, rand)
}

/// Convert internal ToolDef format to Anthropic API tool format.
/// Deferred tools emit a minimal schema to reduce input token usage;
/// the caller must invoke ToolSearch to retrieve the full schema.
///
/// Schemas are passed through [`strip_top_level_combinators`] first because
/// Anthropic returns HTTP 400 on the *entire* request when any tool's
/// `input_schema` carries a top-level `oneOf` / `allOf` / `anyOf`. Stripping
/// those keys is a defensive belt-and-suspenders guard — tools should not
/// emit them at the top level in the first place, but a single bad tool
/// would otherwise break every Anthropic turn.
pub fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            if t.deferred {
                let short_desc = truncate_deferred_description(&t.description);
                json!({
                    "name": encode_tool_name(&t.name),
                    "description": format!(
                        "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                    ),
                    "input_schema": {
                        "type": "object",
                        "properties": {}
                    }
                })
            } else {
                json!({
                    "name": encode_tool_name(&t.name),
                    "description": t.description,
                    "input_schema": strip_top_level_combinators(&t.input_schema)
                })
            }
        })
        .collect()
}

/// Strip top-level `oneOf` / `allOf` / `anyOf` from a JSON Schema so it is
/// accepted by Anthropic's tool `input_schema` validator. Nested usage
/// (e.g. inside `properties.X`) is left untouched — Anthropic only rejects
/// these at the schema's root.
///
/// Returns a cloned, sanitized schema; the input is never mutated. A schema
/// that has none of these keys at the root round-trips byte-for-byte.
pub(crate) fn strip_top_level_combinators(schema: &Value) -> Value {
    let mut out = schema.clone();
    if let Some(map) = out.as_object_mut() {
        for key in ["oneOf", "allOf", "anyOf"] {
            map.remove(key);
        }
    }
    out
}

/// State machine for accumulating SSE content blocks
pub struct StreamState {
    /// Current block type being accumulated
    pub current_block_type: Option<String>,
    /// Accumulated tool input JSON fragments
    pub tool_input_json: String,
    /// Tool use ID for current block
    pub tool_id: String,
    /// Tool name for current block
    pub tool_name: String,
    /// Input tokens from message_start
    pub input_tokens: u64,
    /// Output tokens accumulated
    pub output_tokens: u64,
    /// Cache creation tokens (prompt caching)
    pub cache_creation_tokens: u64,
    /// Cache read tokens (prompt caching)
    pub cache_read_tokens: u64,
}

impl Default for StreamState {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            current_block_type: None,
            tool_input_json: String::new(),
            tool_id: String::new(),
            tool_name: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        }
    }
}

/// Maximum size a single unterminated SSE event block may reach before the
/// parser gives up. M4: a stream that never emits the `\n\n` event delimiter
/// would otherwise grow `buffer` without bound. 1 MiB is far above any
/// legitimate Anthropic SSE event.
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

/// Process the SSE stream from an Anthropic-compatible API
pub async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();
    let mut current_event_type = String::new();
    let mut stream = response.bytes_stream();
    // Decode the byte stream incrementally so a multi-byte codepoint split
    // across TCP chunks is not corrupted into U+FFFD (text and tool-arg JSON).
    let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();
    // E-H3 / D4: track whether a terminal event (Done or in-band Error) was
    // emitted. A stream that closes without one was truncated mid-response —
    // surface it as an error instead of a silent clean turn.
    let mut terminal_seen = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = utf8.push(&chunk);
        buffer.push_str(&text);

        // M4: cap the buffer so a delimiter-less stream cannot exhaust memory.
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "SSE event exceeded {MAX_SSE_BUFFER_BYTES} bytes without a \\n\\n delimiter"
            )));
        }

        // Process complete SSE events (separated by double newlines)
        while let Some(event_end) = buffer.find("\n\n") {
            let event_block = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_block.lines() {
                if let Some(event_type) = line.strip_prefix("event: ") {
                    current_event_type = event_type.to_string();
                } else if let Some(data) = line.strip_prefix("data: ") {
                    dump_response_chunk(debug, data);
                    let events = parse_sse_data(&current_event_type, data, &mut state);
                    for event in events {
                        if matches!(event, LlmEvent::Done { .. } | LlmEvent::Error(_)) {
                            terminal_seen = true;
                        }
                        if tx.send(event).await.is_err() {
                            return Ok(()); // receiver dropped
                        }
                    }
                }
            }
        }
    }

    // E-H3 / D4: the byte stream ended. If no `message_delta` (which carries
    // the `Done`) and no `error` frame was seen, the connection closed
    // before the model finished — a silent truncation. Return Err so the
    // provider's spawn forwards an `LlmEvent::Error` rather than just
    // closing the channel (which the engine mis-reads as a clean turn).
    if !terminal_seen {
        return Err(ProviderError::Connection(
            "Anthropic SSE stream closed before a terminal event \
             (message_delta / error) — response truncated"
                .into(),
        ));
    }

    Ok(())
}

/// wayland#552 — env-gated raw SSE capture. When
/// `WAYLAND_ANTHROPIC_SSE_DUMP` names a file, every SSE event this parser
/// receives is appended as one `event_type\tdata` line BEFORE parsing, so a
/// frame shape the parser silently ignores (unknown block/delta types fall
/// through `_ => {}` arms) is still observable. Diagnostic instrument for
/// model-family rollouts (first user: the claude-fable-5 empty-bash-loop
/// capture); zero overhead when the env var is unset (checked once).
/// SECURITY: the dump contains raw model output — the operator chooses the
/// path and owns the file; nothing is captured by default.
fn sse_dump(event_type: &str, data: &str) {
    use std::io::Write;
    use std::sync::OnceLock;
    static DUMP_PATH: OnceLock<Option<String>> = OnceLock::new();
    let Some(path) = DUMP_PATH
        .get_or_init(|| std::env::var("WAYLAND_ANTHROPIC_SSE_DUMP").ok())
        .as_ref()
    else {
        return;
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    // The dump holds raw model output (may echo secrets / tool args), so
    // create it owner-only rather than umask-default 0644.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(path) {
        let _ = writeln!(f, "{event_type}\t{data}");
    }
}

/// Parse a single SSE data payload into zero or more LlmEvents
pub fn parse_sse_data(event_type: &str, data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    sse_dump(event_type, data);
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            // L1: log malformed `data:` frames (truncated) so a provider
            // emitting subtly broken frames is debuggable instead of
            // silently producing a no-terminal-event truncation downstream.
            let preview: String = data.chars().take(200).collect();
            tracing::warn!(
                target: "wcore_providers::anthropic",
                error = %e,
                payload = %preview,
                "discarding malformed Anthropic SSE data frame"
            );
            return events;
        }
    };

    match event_type {
        "message_start" => {
            if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                state.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                state.cache_creation_tokens =
                    usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                state.cache_read_tokens = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            }
        }

        "content_block_start" => {
            let block = &json["content_block"];
            let block_type = block["type"].as_str().unwrap_or("");
            state.current_block_type = Some(block_type.to_string());

            if block_type == "tool_use" {
                state.tool_id = block["id"].as_str().unwrap_or("").to_string();
                // Decode the wire name back to the canonical tool id so the
                // model's call resolves against the registry (mirrors the
                // encode in `build_tools` / assistant-history `tool_use`).
                state.tool_name = decode_tool_name(block["name"].as_str().unwrap_or(""));
                state.tool_input_json.clear();
            }
        }

        "content_block_delta" => {
            let delta = &json["delta"];
            let delta_type = delta["type"].as_str().unwrap_or("");

            match delta_type {
                "text_delta" => {
                    if let Some(text) = delta["text"].as_str() {
                        events.push(LlmEvent::TextDelta(text.to_string()));
                    }
                }
                "input_json_delta" => {
                    if let Some(partial) = delta["partial_json"].as_str() {
                        // Bound the streaming tool-argument accumulator. A
                        // misbehaving or hostile upstream — including the
                        // MiniMax Anthropic-compatible endpoint, which reuses
                        // this path — could otherwise stream `input_json_delta`
                        // frames without end and OOM the process (the SSE buffer
                        // cap only bounds a single event block, not the
                        // cross-delta accumulation). Past the cap we stop
                        // appending; the truncated buffer then fails the JSON
                        // parse in `content_block_stop`'s existing fail-closed
                        // branch, so the tool call is rejected, not run with
                        // partial input. (deep-sweep F39)
                        const MAX_TOOL_INPUT_JSON_BYTES: usize = 8 * 1024 * 1024;
                        let projected = state.tool_input_json.len() + partial.len();
                        if projected <= MAX_TOOL_INPUT_JSON_BYTES {
                            state.tool_input_json.push_str(partial);
                        }
                    }
                }
                "thinking_delta" => {
                    if let Some(thinking) = delta["thinking"].as_str() {
                        events.push(LlmEvent::ThinkingDelta(thinking.to_string()));
                    }
                }
                _ => {}
            }
        }

        "content_block_stop" => {
            if state.current_block_type.as_deref() == Some("tool_use") {
                // Fail closed: non-empty tool-argument JSON that does not parse
                // (truncation/corruption) must NOT run the tool with empty
                // input — surface an error instead. Genuinely empty arguments
                // (a no-parameter tool) remain a valid empty object.
                let trimmed = state.tool_input_json.trim();
                if trimmed.is_empty() {
                    events.push(LlmEvent::ToolUse {
                        id: state.tool_id.clone(),
                        name: state.tool_name.clone(),
                        input: Value::Object(serde_json::Map::new()),
                        extra: None,
                    });
                } else {
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(input) => events.push(LlmEvent::ToolUse {
                            id: state.tool_id.clone(),
                            name: state.tool_name.clone(),
                            input,
                            extra: None,
                        }),
                        Err(e) => events.push(LlmEvent::Error(format!(
                            "tool-call arguments for '{}' did not parse as JSON: {e}",
                            state.tool_name
                        ))),
                    }
                }
                state.tool_input_json.clear();
            }
            state.current_block_type = None;
        }

        "message_delta" => {
            let delta = &json["delta"];
            let raw = delta["stop_reason"].as_str();
            let (stop_reason, finish_reason) = map_anthropic_stop_reason(raw);

            if let Some(usage) = json.get("usage") {
                state.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
            }

            events.push(LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            });
        }

        "message_stop" => {
            // Stream complete, no action needed
        }

        "error" => {
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown API error");
            events.push(LlmEvent::Error(msg.to_string()));
        }

        _ => {}
    }

    events
}

/// Map an Anthropic-shape `stop_reason` string to internal `StopReason`
/// and protocol-level `FinishReason`.
///
/// Used by:
/// - native Anthropic SSE (`crates/wcore-providers/src/anthropic.rs`)
/// - Vertex AI (`vertex.rs`) — Vertex emits Anthropic SSE verbatim
/// - Bedrock anthropic-passthrough (`bedrock.rs`) — wraps inner SSE
///
/// Unrecognised values (e.g. `"refusal"`, future Anthropic additions) map
/// to `FinishReason::Error` with a stderr warning, instead of silently
/// degrading to `EndTurn` as the pre-Task-F code did.
pub(crate) fn map_anthropic_stop_reason(raw: Option<&str>) -> (StopReason, FinishReason) {
    match raw {
        Some("end_turn") => (StopReason::EndTurn, FinishReason::Stop),
        Some("tool_use") => (StopReason::ToolUse, FinishReason::Stop),
        Some("max_tokens") => (StopReason::MaxTokens, FinishReason::Length),
        Some(other) => {
            eprintln!(
                "[wcore-providers] anthropic: unrecognized stop_reason {other:?}, mapping to FinishReason::Error"
            );
            (StopReason::EndTurn, FinishReason::Error)
        }
        None => {
            eprintln!(
                "[wcore-providers] anthropic: message_delta arrived without stop_reason, mapping to FinishReason::Error"
            );
            (StopReason::EndTurn, FinishReason::Error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use wcore_types::tool::ToolDef;

    /// Compat with merge but no alternation — matches pre-compat behavior
    fn default_compat() -> ProviderCompat {
        ProviderCompat {
            merge_same_role: Some(true),
            ..Default::default()
        }
    }

    // --- build_messages tests ---

    #[test]
    fn test_build_messages_text_only() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello");
    }

    #[test]
    fn test_build_messages_with_tool_use() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: json!({"cmd": "ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_1");
        assert_eq!(content[0]["name"], "bash");
        assert_eq!(content[0]["input"]["cmd"], "ls");
    }

    #[test]
    fn test_build_messages_with_tool_result() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file list".to_string(),
                is_error: false,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user"); // Tool maps to "user"
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[0]["content"], "file list");
        assert_eq!(content[0]["is_error"], false);
    }

    /// wayland#161: thinking blocks must NOT be replayed — they lack the
    /// `signature` Anthropic requires (we never capture it, and a model switch
    /// would invalidate it), so replaying one 400s and strands the conversation.
    #[test]
    fn test_build_messages_drops_thinking_blocks() {
        // A turn with text + thinking keeps only the text.
        let messages = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    thinking: "Let me think...".to_string(),
                },
                ContentBlock::Text {
                    text: "Here is the answer.".to_string(),
                },
            ],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "thinking block must be dropped");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Here is the answer.");
        // No thinking block may survive into the request.
        assert!(content.iter().all(|b| b["type"] != "thinking"));
    }

    /// A turn truncated mid-thinking (only a thinking block) becomes empty and
    /// must be skipped entirely — Anthropic rejects empty `content`. This is the
    /// exact wayland#161 reproduction (truncation then continue/model-switch).
    #[test]
    fn test_build_messages_skips_thinking_only_turn() {
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Thinking {
                    thinking: "interrupted...".to_string(),
                }],
            ),
        ];
        let result = build_messages(&messages, &default_compat());
        // Only the user turn survives; the thinking-only assistant turn is gone.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
    }

    /// wayland#161 accept criterion: a history that was truncated mid-thinking
    /// and then CONTINUED ON A DIFFERENT MODEL replays cleanly — no thinking
    /// block (whose signature would belong to the prior model) reaches the wire,
    /// so Anthropic never returns the `thinking.signature` 400 that strands the
    /// conversation. The engine never captures a signature, so every historical
    /// thinking block is dropped regardless of which model produced it; this
    /// test pins that invariant across the model-switch path specifically.
    #[test]
    fn test_build_messages_drops_thinking_across_model_switch() {
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "long task".to_string(),
                }],
            ),
            // Turn truncated mid-thinking under the OLD model (budget ran out).
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Thinking {
                    thinking: "reasoning minted by the previous model...".to_string(),
                }],
            ),
            // User continues; the session is now on a DIFFERENT model.
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "continue".to_string(),
                }],
            ),
            // The new model's turn also carries thinking + text.
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "new model reasoning...".to_string(),
                    },
                    ContentBlock::Text {
                        text: "done".to_string(),
                    },
                ],
            ),
        ];
        let result = build_messages(&messages, &default_compat());
        // No `thinking` block may appear in ANY message sent to the provider.
        for msg in &result {
            if let Some(content) = msg["content"].as_array() {
                assert!(
                    content.iter().all(|b| b["type"] != "thinking"),
                    "no thinking block may survive a model switch (wayland#161): {msg}"
                );
            }
        }
        // The truncated thinking-only turn collapsed away; the surviving
        // assistant turn keeps only its text.
        let last = result.last().expect("a turn must survive");
        assert_eq!(last["role"], "assistant");
        assert_eq!(last["content"][0]["text"], "done");
    }

    // --- compat-driven behavior tests ---

    #[test]
    fn test_ensure_alternation_inserts_user_filler_before_assistant() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            ensure_alternation: Some(true),
            merge_same_role: Some(true),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[1]["role"], "assistant");
    }

    #[test]
    fn test_ensure_alternation_disabled_no_filler() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            ensure_alternation: Some(false),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
    }

    #[test]
    fn test_merge_same_role_enabled_merges_consecutive_user() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            merge_same_role: Some(true),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn test_merge_same_role_disabled_keeps_separate() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            merge_same_role: Some(false),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_auto_tool_id_generates_id_when_empty() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: String::new(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            auto_tool_id: Some(true),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        let id = content[0]["id"].as_str().unwrap();
        assert!(id.starts_with("toolu_"));
    }

    #[test]
    fn test_auto_tool_id_preserves_existing_id() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "existing_id".into(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            auto_tool_id: Some(true),
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["id"], "existing_id");
    }

    // --- build_tools tests ---

    #[test]
    fn test_build_tools_single() {
        // arrange
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" }
            },
            "required": ["cmd"]
        });
        let tools = vec![ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command".to_string(),
            input_schema: schema.clone(),
            deferred: false,
            server: None,
        }];
        // act
        let result = build_tools(&tools);
        // assert
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "bash");
        assert_eq!(result[0]["description"], "Run a shell command");
        // The schema flows through `strip_top_level_combinators` which is a
        // no-op for a schema that has no top-level `oneOf` / `allOf` / `anyOf`.
        assert_eq!(result[0]["input_schema"], schema);
    }

    #[test]
    fn test_build_tools_empty() {
        // arrange
        let tools: Vec<ToolDef> = vec![];
        // act
        let result = build_tools(&tools);
        // assert
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_tools_deferred_has_empty_schema() {
        let tools = vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
                server: None,
            },
        ];
        let result = build_tools(&tools);

        // Core tool has full input_schema
        assert!(
            result[0]["input_schema"]["properties"]
                .get("path")
                .is_some()
        );

        // Deferred tool has empty input_schema and modified description
        assert!(
            result[1]["input_schema"]["properties"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let desc = result[1]["description"].as_str().unwrap();
        assert!(desc.contains("ToolSearch"));
    }

    // --- parse_sse_data tests ---

    #[test]
    fn test_parse_anthropic_event_text_delta() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"text_delta","text":"Hello"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_tool_use() {
        // arrange
        let mut state = StreamState::new();
        // step 1: content_block_start with tool_use type
        let start_events = parse_sse_data(
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"id1","name":"bash"}}"#,
            &mut state,
        );
        assert!(start_events.is_empty());
        // step 2: content_block_delta with input_json_delta
        let delta_events = parse_sse_data(
            "content_block_delta",
            r#"{"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            &mut state,
        );
        assert!(delta_events.is_empty());
        // step 3: content_block_stop emits the ToolUse event
        let events = parse_sse_data("content_block_stop", r#"{}"#, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "id1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_stop() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        // act
        let events = parse_sse_data("message_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(*finish_reason, FinishReason::Stop);
                assert_eq!(usage.output_tokens, 42);
            }
            _ => panic!("expected Done"),
        }
    }

    // --- map_anthropic_stop_reason (Task F) -------------------------------

    #[test]
    fn test_map_anthropic_end_turn_to_stop() {
        let (sr, fr) = map_anthropic_stop_reason(Some("end_turn"));
        assert_eq!(sr, StopReason::EndTurn);
        assert_eq!(fr, FinishReason::Stop);
    }

    #[test]
    fn test_map_anthropic_tool_use_to_stop() {
        let (sr, fr) = map_anthropic_stop_reason(Some("tool_use"));
        assert_eq!(sr, StopReason::ToolUse);
        assert_eq!(fr, FinishReason::Stop);
    }

    #[test]
    fn test_map_anthropic_max_tokens_to_length() {
        let (sr, fr) = map_anthropic_stop_reason(Some("max_tokens"));
        assert_eq!(sr, StopReason::MaxTokens);
        assert_eq!(fr, FinishReason::Length);
    }

    #[test]
    fn test_map_anthropic_refusal_to_error() {
        // "refusal" is a real Anthropic stop_reason for safety blocks.
        let (_, fr) = map_anthropic_stop_reason(Some("refusal"));
        assert_eq!(fr, FinishReason::Error);
    }

    #[test]
    fn test_map_anthropic_unknown_to_error() {
        let (_, fr) = map_anthropic_stop_reason(Some("garbage_future_value"));
        assert_eq!(fr, FinishReason::Error);
    }

    #[test]
    fn test_map_anthropic_none_to_error() {
        let (_, fr) = map_anthropic_stop_reason(None);
        assert_eq!(fr, FinishReason::Error);
    }

    #[test]
    fn test_parse_anthropic_event_thinking() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"thinking_delta","thinking":"reasoning step"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "reasoning step"),
            _ => panic!("expected ThinkingDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_unknown_type() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{}"#;
        // act
        let events = parse_sse_data("unknown_event", data, &mut state);
        // assert
        assert!(events.is_empty());
    }

    #[test]
    fn build_tools_strips_top_level_combinators_from_input_schema() {
        // A real prod regression: the transcription tool used to carry a
        // top-level `oneOf` for mutual-exclusion enforcement, which made
        // Anthropic return HTTP 400 for the entire request. `build_tools`
        // now sanitises every tool's schema so a single offending tool can
        // no longer brick the whole turn.
        use wcore_types::tool::ToolDef;
        let tool = ToolDef {
            name: "transcribe_audio".into(),
            description: "test".into(),
            server: None,
            deferred: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "audio_path": {"type": "string"},
                    "audio_url": {"type": "string"}
                },
                "oneOf": [
                    {"required": ["audio_path"]},
                    {"required": ["audio_url"]}
                ]
            }),
        };
        let result = build_tools(&[tool]);
        assert!(
            result[0]["input_schema"].get("oneOf").is_none(),
            "top-level oneOf must be stripped: {}",
            result[0]["input_schema"]
        );
        // The rest of the schema survives.
        assert_eq!(result[0]["input_schema"]["type"], "object");
        assert!(result[0]["input_schema"]["properties"]["audio_path"].is_object());
    }

    #[test]
    fn build_tools_preserves_nested_combinators() {
        // Only TOP-LEVEL oneOf/allOf/anyOf trip Anthropic; nested usage
        // (inside `properties.X`) is fine and must survive.
        use wcore_types::tool::ToolDef;
        let tool = ToolDef {
            name: "spotify".into(),
            description: "test".into(),
            server: None,
            deferred: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "oneOf": [{"type": "string"}, {"type": "boolean"}]
                    }
                }
            }),
        };
        let result = build_tools(&[tool]);
        assert!(
            result[0]["input_schema"]["properties"]["state"]["oneOf"].is_array(),
            "nested oneOf must survive: {}",
            result[0]["input_schema"]
        );
    }

    // --- tool-name sanitization (Anthropic mirror of OpenAI #297) ---------

    /// Anthropic requires `tools[N].custom.name` to match
    /// `^[a-zA-Z0-9_-]{1,128}$` and 400s the ENTIRE request otherwise, so a
    /// single MCP tool with a `:`/`.` in its name (e.g. `Browser::execute`)
    /// used to abort every tool-calling turn. `build_tools` now emits a
    /// wire-legal encoded name for both the deferred and full-schema branches.
    #[test]
    fn build_tools_sanitizes_invalid_names() {
        use wcore_types::tool::ToolDef;
        let tools = vec![
            ToolDef {
                name: "Browser::execute".into(),
                description: "full".into(),
                server: None,
                deferred: false,
                input_schema: json!({"type": "object", "properties": {}}),
            },
            ToolDef {
                name: "com.microsoft-markitdown".into(),
                description: "deferred".into(),
                server: None,
                deferred: true,
                input_schema: json!({"type": "object", "properties": {}}),
            },
        ];
        let result = build_tools(&tools);
        for (i, orig) in ["Browser::execute", "com.microsoft-markitdown"]
            .into_iter()
            .enumerate()
        {
            let wire = result[i]["name"].as_str().unwrap();
            assert_ne!(wire, orig, "invalid name must be encoded: {wire}");
            assert!(is_anthropic_name_legal(wire), "not wire-legal: {wire}");
        }
    }

    /// A charset-clean but over-length MCP name (real shape) is clamped to a
    /// wire-legal ≤ 64-char name by `build_tools` — this is the OpenAI-64 /
    /// Flux-kimi length 400 that survived the charset-only #297 fix.
    #[test]
    fn build_tools_clamps_over_length_names() {
        use wcore_types::tool::ToolDef;
        let long =
            "mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels";
        let tools = vec![ToolDef {
            name: long.into(),
            description: "test".into(),
            server: None,
            deferred: false,
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        let result = build_tools(&tools);
        let wire = result[0]["name"].as_str().unwrap();
        assert!(is_anthropic_name_legal(wire), "not wire-legal: {wire}");
        assert!(wire.len() <= 64, "name must be clamped to ≤64: {wire}");
    }

    /// End-to-end round-trip: the encoded name emitted by `build_tools` decodes
    /// back to the canonical id when the model calls the tool (parsed via
    /// `content_block_start`), for both the charset and the length regimes.
    #[test]
    fn tool_name_round_trips_through_build_and_parse() {
        use wcore_types::tool::ToolDef;
        for orig in [
            "Browser::execute",
            "mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels",
        ] {
            let tools = vec![ToolDef {
                name: orig.into(),
                description: "t".into(),
                server: None,
                deferred: false,
                input_schema: json!({"type": "object", "properties": {}}),
            }];
            // Encode happens in build_tools; grab the wire name.
            let wire = build_tools(&tools)[0]["name"].as_str().unwrap().to_string();
            // Model streams a tool_use referencing that wire name.
            let mut state = StreamState::new();
            let payload = json!({
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": wire}
            })
            .to_string();
            parse_sse_data("content_block_start", &payload, &mut state);
            assert_eq!(state.tool_name, orig, "round-trip failed for {orig}");
        }
    }

    fn is_anthropic_name_legal(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}
