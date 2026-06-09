// Google Gemini native provider — Generative Language API.
//
// W11 (debt-register B.4-Gemini, F.2 carryover from rebrand chain).
//
// This is the FIRST-CLASS native path. The OpenAI-compat surface in
// `openai.rs` (with the `delta.extra_content.google.thought` routing W6
// added in B.3) remains supported as a fallback for users who can only
// reach Gemini through a compat shim.
//
// Why native:
// - `thoughtSignature` round-trip across turns (the OpenAI-compat surface
//   loses it).
// - Real `tools: [{ functionDeclarations: [...] }]` shape instead of
//   coercing through OpenAI `tool_calls`.
// - Native multimodal (`parts: [{ inlineData: { mimeType, data } }]`) —
//   the compat path base64-stuffs everything into text.
// - `safetySettings` exposed as first-class config.
//
// Endpoint:
//   POST https://generativelanguage.googleapis.com/v1beta/models/<MODEL>:streamGenerateContent
//        ?key=<API_KEY>&alt=sse
//
// Streaming response is standard SSE with `data: {...}` chunks containing
// `{ candidates: [{ content: { parts: [...] }, finishReason }], usageMetadata }`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::tool::{ToolDef, truncate_deferred_description};

use crate::key_rotation::{KeyPool, split_keys};
use crate::retry::builder_send_with_retry;
use crate::{
    LlmProvider, ProviderError, dump_request_body, dump_response_chunk, reset_response_dump,
};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

/// Default Google Generative Language API base URL.
pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// One safety category + threshold pair. The Gemini API accepts a list of
/// these via `safetySettings`. Both fields are passed verbatim — the API
/// validates the values.
#[derive(Debug, Clone)]
pub struct SafetySetting {
    pub category: String,
    pub threshold: String,
}

pub struct GeminiProvider {
    client: wcore_egress::EgressClient,
    /// Rotation pool over one-or-more API keys. A single configured key yields
    /// a one-element pool — behavior identical to the pre-rotation path. Wrapped
    /// in `Arc<Mutex<…>>` so `&self` request methods can rotate/demote keys.
    keys: Arc<Mutex<KeyPool>>,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
    /// Optional safety overrides. None ⇒ provider defaults apply.
    safety_settings: Vec<SafetySetting>,
}

impl GeminiProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        let base = if base_url.is_empty() {
            DEFAULT_GEMINI_BASE_URL.to_string()
        } else {
            base_url.to_string()
        };
        Self {
            client: crate::http_client::build(),
            keys: Arc::new(Mutex::new(KeyPool::new(split_keys(api_key)))),
            base_url: base,
            compat,
            debug,
            safety_settings: Vec::new(),
        }
    }

    pub fn with_safety_settings(mut self, settings: Vec<SafetySetting>) -> Self {
        self.safety_settings = settings;
        self
    }

    /// Select the API key to authenticate the next request. Delegates to
    /// [`KeyPool::next_key`] (prefers the last-good key, rotates round-robin on
    /// failure, skips keys in cooldown). Returns [`ProviderError::MissingApiKey`]
    /// when no key is configured or every key is cooling.
    fn select_key(&self) -> Result<String, ProviderError> {
        let mut pool = self.keys.lock().expect("key pool mutex poisoned");
        pool.next_key()
            .map(str::to_string)
            .ok_or(ProviderError::MissingApiKey)
    }

    /// Promote `key` to last-good after a successful (2xx) response.
    fn mark_key_success(&self, key: &str) {
        self.keys
            .lock()
            .expect("key pool mutex poisoned")
            .mark_success(key);
    }

    /// Demote `key` for the cooldown window after an auth/rate-limit failure
    /// (401/403/429), so the next request rotates to another key.
    fn mark_key_failure(&self, key: &str) {
        self.keys
            .lock()
            .expect("key pool mutex poisoned")
            .mark_failure(key);
    }

    fn build_headers(&self, key: &str) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // H-2 / secrets-26: the API key rides in the `x-goog-api-key` header,
        // NOT the `?key=` query string. A key in the URL leaks into reqwest's
        // error `Display` (which appends ` for url (…?key=<KEY>)`), into the
        // `[retry]` tracing warning, into `LlmEvent::Error`, and across any 302
        // redirect. The header form is stripped cross-host by reqwest and
        // never appears in a URL-bearing error. Gemini accepts the key on
        // either surface; the header is the only one that is not loggable.
        let mut key = HeaderValue::from_str(key).map_err(|e| {
            ProviderError::Connection(format!("invalid Gemini API key header value: {e}"))
        })?;
        key.set_sensitive(true);
        headers.insert("x-goog-api-key", key);
        Ok(headers)
    }

    fn build_url(&self, model: &str) -> String {
        // `:streamGenerateContent` + `alt=sse` is the SSE-framed streaming
        // endpoint. Without `alt=sse` the server returns one large JSON
        // array, not an event stream.
        //
        // H-2 / providers-net-32: the API key is NO LONGER in the query string
        // (it moved to the `x-goog-api-key` header in `build_headers`). The
        // model segment is percent-encoded so an exotic model id cannot inject
        // extra path/query structure into the URL.
        format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/'),
            encode_path_segment(model),
        )
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        // Native Gemini callers put the system prompt in Role::System; pass
        // an empty extra_system so we don't double-stuff it.
        build_gemini_body(request, &self.compat, &self.safety_settings, "")
    }
}

/// Percent-encode a single URL path segment (H-2 / providers-net-32).
///
/// We don't depend on the `percent-encoding` crate here, so this minimal
/// encoder passes through the RFC 3986 "unreserved" set plus the small set of
/// sub-delims that are legal and meaningful in a Gemini model id (`.`, `-`,
/// `_`, `~`, `:` — Bedrock-style `model:tag`). Everything else — including
/// `/`, `?`, `#`, `&`, `=`, space, and control bytes — is `%`-escaped so an
/// exotic or hostile `model` value cannot inject extra path or query
/// structure into the request URL.
fn encode_path_segment(s: &str) -> String {
    fn is_unreserved(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b':')
    }
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit((b >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((b & 0x0f) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

/// Build the Gemini request body. Free function (not a method) so the
/// Vertex-Gemini path can reuse it without instantiating a `GeminiProvider`.
/// `safety_settings` may be empty; `compat` controls `merge_same_role`.
///
/// `extra_system` is prepended onto whatever `systemInstruction` we extract
/// from `Role::System` messages. The native Gemini provider passes `""`
/// here (its callers put the system prompt in `Role::System`); the
/// Vertex-Gemini path passes `request.system` because callers writing
/// against the Anthropic-on-Vertex convention populate that field as a
/// plain string and would otherwise see the system prompt silently dropped.
pub(crate) fn build_gemini_body(
    request: &LlmRequest,
    compat: &ProviderCompat,
    safety_settings: &[SafetySetting],
    extra_system: &str,
) -> Value {
    let (mut system_instruction, contents) = build_contents(&request.messages, compat);
    if !extra_system.is_empty() {
        system_instruction = Some(match system_instruction {
            Some(existing) => format!("{extra_system}\n\n{existing}"),
            None => extra_system.to_string(),
        });
    }

    let mut generation_config = json!({
        "maxOutputTokens": request.max_tokens,
    });
    if let Some(effort) = &request.reasoning_effort
        && !effort.is_empty()
    {
        // Gemini 2.5 thinking budget is controlled via
        // `thinkingConfig.thinkingBudget` (token count) or
        // `thinkingConfig.includeThoughts`. We translate the abstract
        // effort levels to concrete budget knobs:
        //   - "low"    → 1024 tokens, thoughts off (cheap reasoning)
        //   - "medium" → 8192 tokens, thoughts ON
        //   - "high"   → 24576 tokens, thoughts ON
        // Anything else: include thoughts but don't pin a budget.
        let (budget, include) = match effort.as_str() {
            "low" => (Some(1024_u32), false),
            "medium" => (Some(8192_u32), true),
            "high" => (Some(24576_u32), true),
            _ => (None, true),
        };
        let mut thinking_cfg = json!({ "includeThoughts": include });
        if let Some(b) = budget {
            thinking_cfg["thinkingBudget"] = json!(b);
        }
        generation_config["thinkingConfig"] = thinking_cfg;
    }

    // Output-side optimization (Part A): UNION the request's fluff stop
    // sequences into Gemini's `generationConfig.stopSequences`, preserving any
    // the caller already placed there. Skipped when empty so back-compatible
    // callers emit no stop field.
    if !request.stop_sequences.is_empty() {
        match generation_config
            .get_mut("stopSequences")
            .and_then(Value::as_array_mut)
        {
            Some(existing) => {
                for s in &request.stop_sequences {
                    existing.push(json!(s));
                }
            }
            None => {
                generation_config["stopSequences"] = json!(request.stop_sequences);
            }
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": generation_config,
    });

    if let Some(sys) = system_instruction {
        body["systemInstruction"] = json!({
            "parts": [{ "text": sys }],
        });
    }

    if !request.tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": build_function_declarations(&request.tools),
        }]);
    }

    if !safety_settings.is_empty() {
        body["safetySettings"] = json!(
            safety_settings
                .iter()
                .map(|s| json!({
                    "category": s.category,
                    "threshold": s.threshold,
                }))
                .collect::<Vec<_>>()
        );
    }

    body
}

/// Convert internal messages to Gemini `contents` + extract system instruction.
///
/// Gemini doesn't accept system messages inline — they go on a separate
/// top-level `systemInstruction` field. `Role::Tool` results become
/// `parts: [{ functionResponse: {...} }]` under a `user` role (Gemini's
/// convention for tool results).
pub(crate) fn build_contents(
    messages: &[Message],
    compat: &ProviderCompat,
) -> (Option<String>, Vec<Value>) {
    // Pull every system message into a single concatenated systemInstruction.
    let system_parts: Vec<String> = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| {
            m.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
        })
        .collect();
    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    let mut contents: Vec<Value> = Vec::new();

    for msg in messages {
        let role_str = match msg.role {
            // Gemini uses "user" / "model" — tool results also go under "user".
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
            Role::System => continue, // already pulled into systemInstruction
        };

        let mut parts: Vec<Value> = Vec::new();
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    let mut t = text.clone();
                    if let Some(patterns) = &compat.strip_patterns {
                        for p in patterns {
                            t = t.replace(p, "");
                        }
                    }
                    if !t.is_empty() {
                        parts.push(json!({ "text": t }));
                    }
                }
                ContentBlock::ToolUse {
                    name, input, extra, ..
                } => {
                    let mut part = json!({
                        "functionCall": {
                            "name": name,
                            "args": input,
                        }
                    });
                    // Round-trip the thoughtSignature (and any other Gemini
                    // metadata) so multi-turn reasoning chains stay intact.
                    if let Some(extra_val) = extra
                        && let Some(sig) = extra_val.get("thoughtSignature")
                        && let Some(sig_str) = sig.as_str()
                    {
                        part["thoughtSignature"] = json!(sig_str);
                    }
                    parts.push(part);
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // Gemini's functionResponse uses the function NAME, not the
                    // call ID — but we don't carry the name back through the
                    // engine's ToolResult block. Use the id as a stable label;
                    // the model only cares about pairing by position.
                    let mut response_obj = json!({ "content": content });
                    if *is_error {
                        response_obj["isError"] = json!(true);
                    }
                    parts.push(json!({
                        "functionResponse": {
                            "name": tool_use_id,
                            "response": response_obj,
                        }
                    }));
                }
                ContentBlock::Thinking { thinking } => {
                    // Round-trip thinking text as a thought-flagged part so the
                    // model has the prior reasoning context. The signature (if
                    // any) was preserved on the corresponding ToolUse block.
                    parts.push(json!({
                        "text": thinking,
                        "thought": true,
                    }));
                }
            }
        }

        if parts.is_empty() {
            continue;
        }

        // Merge consecutive same-role turns when compat says so. Gemini
        // tolerates either shape; merging keeps the on-wire payload tidy.
        if compat.merge_same_role()
            && let Some(last) = contents.last_mut()
            && last["role"].as_str() == Some(role_str)
            && let Some(arr) = last["parts"].as_array_mut()
        {
            arr.extend(parts);
            continue;
        }

        contents.push(json!({
            "role": role_str,
            "parts": parts,
        }));
    }

    (system_instruction, contents)
}

/// Convert internal ToolDef list to Gemini functionDeclarations.
///
/// `pub(crate)` so the Vertex-Gemini path (`vertex.rs`) can reuse it. Vertex
/// Gemini speaks the exact same body shape as the public Generative Language
/// API — only the URL and auth header differ.
///
/// F-008: each tool's `parameters` schema is passed through
/// [`sanitize_schema_for_gemini`] before serialization to strip constructs
/// that Gemini's proto-derived schema validator rejects:
/// - `additionalProperties` (any value — boolean or object)
/// - Array-form `type` like `["string", "array"]` (collapsed to first non-null)
pub(crate) fn build_function_declarations(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            if t.deferred {
                let short_desc = truncate_deferred_description(&t.description);
                json!({
                    "name": t.name,
                    "description": format!(
                        "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                    ),
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                })
            } else {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": sanitize_schema_for_gemini(&t.input_schema),
                })
            }
        })
        .collect()
}

/// Strip JSON Schema constructs that Gemini's proto-derived validator rejects.
///
/// Gemini uses a proto3-derived schema validator for `functionDeclarations[*].parameters`.
/// Two classes of legal JSON Schema fail validation:
///
/// 1. `additionalProperties` — any value (Gemini has no proto field for it).
/// 2. `type: ["string", "array"]` union form — proto expects a single scalar
///    string for `type`, not an array.
///
/// This function walks the schema value recursively and fixes both. It does
/// NOT strip other keywords (`enum`, `description`, `default`, `required`,
/// `format`, `minimum`, etc.) — those are accepted by Gemini.
///
/// `pub(crate)` so tests in this module and Vertex can call it directly.
pub(crate) fn sanitize_schema_for_gemini(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                match k.as_str() {
                    // F-008a: strip additionalProperties entirely.
                    "additionalProperties" => {}
                    // F-008b: array-form type → collapse to single string.
                    "type" if v.is_array() => {
                        let arr = v.as_array().unwrap();
                        // Pick first non-null string type; fall back to "string".
                        let chosen = arr
                            .iter()
                            .filter_map(Value::as_str)
                            .find(|t| *t != "null")
                            .unwrap_or("string");
                        out.insert("type".to_string(), Value::String(chosen.to_string()));
                    }
                    // Recurse into any nested object/array value.
                    _ => {
                        out.insert(k.clone(), sanitize_schema_for_gemini(v));
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_schema_for_gemini).collect()),
        other => other.clone(),
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let key = self.select_key()?;
        let response = builder_send_with_retry(
            self.client
                .post(&url)
                .headers(self.build_headers(&key)?)
                .json(&body),
        )
        .await?;

        let status = response.status();
        if !status.is_success() {
            // Demote this key on auth / rate-limit failures so the next request
            // rotates to another key in the pool (no-op for a single key).
            if matches!(status.as_u16(), 401 | 403 | 429) {
                self.mark_key_failure(&key);
            }
            // E-H1 / L3: capture headers before `.text()` consumes the body
            // so a 429 can honour `Retry-After` (header, then nested body).
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: crate::retry::resolve_retry_after_ms(&headers, &body_text),
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        // 2xx: this key works — make it sticky for subsequent requests.
        self.mark_key_success(&key);

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

/// Streaming-state accumulator for a single Gemini response.
#[derive(Default)]
pub(crate) struct GeminiStreamState {
    /// Final usageMetadata, if any chunk emitted one.
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    /// Carried over the whole response. Gemini emits the signature on the
    /// chunk that produces a functionCall; if multiple calls appear in one
    /// response we keep the last one (it's per-turn, not per-call).
    final_finish_reason: Option<String>,
    /// True if at least one functionCall part was emitted. Drives the
    /// StopReason mapping when finishReason is "STOP" but tool calls exist.
    saw_tool_call: bool,
}

/// Parse the SSE stream from `streamGenerateContent?alt=sse`.
///
/// `pub(crate)` so the Vertex-Gemini path (`vertex.rs`) can reuse it. Vertex
/// emits the identical SSE shape as the public Generative Language API.
pub(crate) async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = GeminiStreamState::default();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    // E-H3 / D4: track whether an in-band Error frame was forwarded. The
    // terminal `Done` is emitted after the loop from `final_finish_reason`;
    // a stream that closes with neither was truncated.
    let mut error_seen = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // M4: cap the buffer so a delimiter-less stream cannot exhaust memory.
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "SSE frame exceeded {MAX_SSE_BUFFER_BYTES} bytes without a \\n\\n delimiter"
            )));
        }

        // SSE frame boundary is "\n\n".
        while let Some(end) = buffer.find("\n\n") {
            let frame = buffer[..end].to_string();
            buffer = buffer[end + 2..].to_string();

            for line in frame.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    dump_response_chunk(debug, data);
                    let events = parse_sse_chunk(data, &mut state);
                    for event in events {
                        if matches!(event, LlmEvent::Error(_)) {
                            error_seen = true;
                        }
                        if tx.send(event).await.is_err() {
                            return Ok(()); // receiver dropped
                        }
                    }
                }
            }
        }
    }

    // End of stream — emit Done if the server ever set a finishReason.
    // (Gemini does not send a sentinel like `data: [DONE]`; the stream
    // simply terminates after the last candidate chunk.)
    if let Some(raw) = state.final_finish_reason.take() {
        let (stop_reason, finish_reason) = map_gemini_finish_reason(&raw, state.saw_tool_call);
        let _ = tx
            .send(LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: state.cache_read_tokens,
                },
            })
            .await;
        return Ok(());
    }

    // E-H3 / D4: stream ended with no `finishReason` and no error frame —
    // a silent truncation. Return Err so the provider's spawn forwards an
    // `LlmEvent::Error` rather than closing the channel cleanly.
    if !error_seen {
        return Err(ProviderError::Connection(
            "Gemini SSE stream closed before a finishReason — response truncated".into(),
        ));
    }

    Ok(())
}

/// Maximum size a single unterminated SSE frame may reach (M4). 1 MiB is far
/// above any legitimate Gemini SSE frame.
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

/// Parse one SSE `data:` payload into zero or more `LlmEvent`s.
pub(crate) fn parse_sse_chunk(data: &str, state: &mut GeminiStreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            // L1: log malformed `data:` frames (truncated) instead of
            // silently dropping them — a provider emitting subtly broken
            // frames should be debuggable.
            let preview: String = data.chars().take(200).collect();
            tracing::warn!(
                target: "wcore_providers::gemini",
                error = %e,
                payload = %preview,
                "discarding malformed Gemini SSE data frame"
            );
            return events;
        }
    };

    // Usage metadata may appear on any chunk; latest wins.
    if let Some(usage) = json.get("usageMetadata") {
        if let Some(v) = usage.get("promptTokenCount").and_then(Value::as_u64) {
            state.input_tokens = v;
        }
        if let Some(v) = usage.get("candidatesTokenCount").and_then(Value::as_u64) {
            state.output_tokens = v;
        }
        // Gemini reports cached input as `cachedContentTokenCount`. The
        // engine's TokenUsage models that as `cache_read_tokens`.
        if let Some(v) = usage.get("cachedContentTokenCount").and_then(Value::as_u64) {
            state.cache_read_tokens = v;
        }
    }

    let Some(candidate) = json["candidates"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    if let Some(parts) = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            // Thought parts (`thought: true`) carry reasoning text. The
            // accompanying `thoughtSignature` is captured on the tool-call
            // part (or, when there is no tool call, simply observed; the
            // public API at present only exposes signatures on calls).
            let is_thought = part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                if is_thought {
                    events.push(LlmEvent::ThinkingDelta(text.to_string()));
                } else {
                    events.push(LlmEvent::TextDelta(text.to_string()));
                }
            }

            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input = call.get("args").cloned().unwrap_or(json!({}));

                // Capture thoughtSignature so the engine can round-trip it
                // on the next turn (LlmEvent::ToolUse.extra). Gemini emits
                // the signature on the same part as the functionCall.
                let extra = part
                    .get("thoughtSignature")
                    .and_then(Value::as_str)
                    .map(|s| json!({ "thoughtSignature": s }));

                // Gemini doesn't ship an explicit call_id. Synthesize a
                // stable one so the engine's ToolUse-pairing machinery
                // (which uses the id as the dedup key) doesn't collide
                // across multiple calls in one turn.
                let id = make_tool_id(&name, state);

                state.saw_tool_call = true;
                events.push(LlmEvent::ToolUse {
                    id,
                    name,
                    input,
                    extra,
                });
            }
        }
    }

    if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str)
        && !reason.is_empty()
    {
        state.final_finish_reason = Some(reason.to_string());
    }

    events
}

/// Synthesize a deterministic tool-call ID. Gemini doesn't send one; the
/// engine pairs `tool_use` ⇄ `tool_result` by ID, so we need a stable label.
fn make_tool_id(name: &str, state: &GeminiStreamState) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Include the running output-token count as a low-cost tiebreaker for
    // multiple calls within one chunk.
    format!("gemini_call_{}_{}_{}", name, ts, state.output_tokens)
}

/// Map Gemini's `finishReason` to internal `StopReason` + protocol `FinishReason`.
///
/// Unknown values map to `FinishReason::Error` (matches Anthropic's
/// `refusal` handling — never silently degrade).
pub(crate) fn map_gemini_finish_reason(
    raw: &str,
    saw_tool_call: bool,
) -> (StopReason, FinishReason) {
    match raw {
        "STOP" => {
            if saw_tool_call {
                (StopReason::ToolUse, FinishReason::Stop)
            } else {
                (StopReason::EndTurn, FinishReason::Stop)
            }
        }
        "MAX_TOKENS" => (StopReason::MaxTokens, FinishReason::Length),
        // SAFETY, RECITATION, BLOCKLIST, PROHIBITED_CONTENT, SPII, OTHER:
        // model refused or was cut off by a policy filter. `FinishReason::Error`
        // surfaces the signal to the host instead of fake-completing the turn.
        other => {
            eprintln!(
                "[wcore-providers] gemini: non-success finishReason {other:?}, mapping to FinishReason::Error"
            );
            (StopReason::EndTurn, FinishReason::Error)
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests — pure-function coverage for parse/build paths.
// Integration tests with a mock server live in tests/gemini_test.rs.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::tool::ToolDef;

    fn compat() -> ProviderCompat {
        ProviderCompat {
            merge_same_role: Some(true),
            ..Default::default()
        }
    }

    fn make_request_with_messages(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: String::new(),
            messages,
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
        }
    }

    #[test]
    fn build_contents_text_only_user_message() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "Hi".into() }],
        )];
        let (sys, contents) = build_contents(&messages, &compat());
        assert!(sys.is_none());
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["text"], "Hi");
    }

    #[test]
    fn build_contents_extracts_system_to_top_level() {
        let messages = vec![
            Message::new(
                Role::System,
                vec![ContentBlock::Text {
                    text: "Be terse.".into(),
                }],
            ),
            Message::new(Role::User, vec![ContentBlock::Text { text: "Hi".into() }]),
        ];
        let (sys, contents) = build_contents(&messages, &compat());
        assert_eq!(sys.as_deref(), Some("Be terse."));
        // System message must NOT appear in the contents array.
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
    }

    #[test]
    fn build_contents_maps_assistant_to_model_role() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Hello".into(),
            }],
        )];
        let (_sys, contents) = build_contents(&messages, &compat());
        assert_eq!(contents[0]["role"], "model");
    }

    #[test]
    fn build_contents_tool_use_renders_function_call() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "x".into(),
                name: "Read".into(),
                input: json!({"path": "/tmp/x"}),
                extra: None,
            }],
        )];
        let (_, contents) = build_contents(&messages, &compat());
        let part = &contents[0]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "Read");
        assert_eq!(part["functionCall"]["args"]["path"], "/tmp/x");
    }

    #[test]
    fn build_contents_round_trips_thought_signature_on_function_call() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "x".into(),
                name: "Read".into(),
                input: json!({}),
                extra: Some(json!({"thoughtSignature": "sig-abc"})),
            }],
        )];
        let (_, contents) = build_contents(&messages, &compat());
        let part = &contents[0]["parts"][0];
        assert_eq!(part["thoughtSignature"], "sig-abc");
    }

    #[test]
    fn build_contents_tool_result_becomes_function_response_under_user_role() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "Read".into(),
                content: "file body".into(),
                is_error: false,
            }],
        )];
        let (_, contents) = build_contents(&messages, &compat());
        assert_eq!(contents[0]["role"], "user");
        let part = &contents[0]["parts"][0];
        assert_eq!(part["functionResponse"]["name"], "Read");
        assert_eq!(part["functionResponse"]["response"]["content"], "file body");
    }

    #[test]
    fn build_contents_merges_consecutive_same_role() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let (_, contents) = build_contents(&messages, &compat());
        assert_eq!(contents.len(), 1);
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn build_request_body_carries_max_tokens_under_generation_config() {
        let provider = GeminiProvider::new(
            "k",
            DEFAULT_GEMINI_BASE_URL,
            compat(),
            DebugConfig::default(),
        );
        let request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        let body = provider.build_request_body(&request);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1024);
    }

    // --- Output-side opt (Part A): stop_sequences ->
    //     generationConfig.stopSequences ---

    #[test]
    fn build_request_body_omits_stop_sequences_when_empty() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        let body = provider.build_request_body(&request);
        assert!(
            body["generationConfig"].get("stopSequences").is_none(),
            "empty stop_sequences must emit no stopSequences field (back-compat)"
        );
    }

    #[test]
    fn build_request_body_emits_stop_sequences_under_generation_config() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        request.stop_sequences = vec!["\n\nLet me know if".into(), "\n\nFeel free to".into()];
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["generationConfig"]["stopSequences"],
            json!(["\n\nLet me know if", "\n\nFeel free to"]),
            "Gemini must emit stops under generationConfig.stopSequences"
        );
    }

    #[test]
    fn build_request_body_emits_system_instruction_when_system_present() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let request = make_request_with_messages(vec![
            Message::new(
                Role::System,
                vec![ContentBlock::Text {
                    text: "Be terse.".into(),
                }],
            ),
            Message::new(Role::User, vec![ContentBlock::Text { text: "Hi".into() }]),
        ]);
        let body = provider.build_request_body(&request);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be terse.");
    }

    #[test]
    fn build_request_body_emits_function_declarations_when_tools_present() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "Hi".into() }],
        )]);
        request.tools = vec![ToolDef {
            name: "Read".into(),
            description: "read a file".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
        }];
        let body = provider.build_request_body(&request);
        let decls = &body["tools"][0]["functionDeclarations"];
        assert_eq!(decls[0]["name"], "Read");
        assert_eq!(
            decls[0]["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[test]
    fn build_request_body_emits_safety_settings_when_configured() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default())
            .with_safety_settings(vec![SafetySetting {
                category: "HARM_CATEGORY_HARASSMENT".into(),
                threshold: "BLOCK_ONLY_HIGH".into(),
            }]);
        let request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "Hi".into() }],
        )]);
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["safetySettings"][0]["category"],
            "HARM_CATEGORY_HARASSMENT"
        );
        assert_eq!(body["safetySettings"][0]["threshold"], "BLOCK_ONLY_HIGH");
    }

    #[test]
    fn build_request_body_translates_reasoning_effort_to_thinking_config() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "Hi".into() }],
        )]);
        request.reasoning_effort = Some("medium".into());
        let body = provider.build_request_body(&request);
        let cfg = &body["generationConfig"]["thinkingConfig"];
        assert_eq!(cfg["thinkingBudget"], 8192);
        assert_eq!(cfg["includeThoughts"], true);
    }

    #[test]
    fn build_url_embeds_model_but_never_the_api_key() {
        // H-2 / providers-net-32: the model is in the path, `alt=sse` is the
        // only query param, and the key must NOT appear anywhere in the URL.
        let provider = GeminiProvider::new(
            "my-key",
            DEFAULT_GEMINI_BASE_URL,
            compat(),
            DebugConfig::default(),
        );
        let url = provider.build_url("gemini-2.5-pro");
        assert!(url.contains("gemini-2.5-pro:streamGenerateContent"));
        assert!(url.contains("alt=sse"));
        assert!(
            !url.contains("key="),
            "the API key must never appear in the URL, got: {url}"
        );
        assert!(
            !url.contains("my-key"),
            "the key value must never appear in the URL, got: {url}"
        );
    }

    #[test]
    fn build_headers_carries_key_in_x_goog_api_key() {
        // H-2 / secrets-26: the key now travels in the `x-goog-api-key`
        // header, marked sensitive so it is redacted from header dumps.
        let provider = GeminiProvider::new("my-key", "", compat(), DebugConfig::default());
        let selected = provider.select_key().expect("single key selects");
        let headers = provider.build_headers(&selected).expect("headers build");
        let key = headers
            .get("x-goog-api-key")
            .expect("x-goog-api-key header must be set");
        assert!(key.is_sensitive(), "the key header must be sensitive");
        assert_eq!(key.to_str().unwrap(), "my-key");
    }

    /// Multi-key rotation: a demoted key rotates to the other key, and a
    /// succeeded key sticks. Single key is the degenerate unchanged case.
    #[test]
    fn multi_key_rotation_demotes_failing_key_then_succeeds() {
        let provider = GeminiProvider::new("key-a, key-b", "", compat(), DebugConfig::default());
        let first = provider.select_key().expect("a key is available");
        assert!(first == "key-a" || first == "key-b");

        provider.mark_key_failure(&first);
        let second = provider.select_key().expect("rotation finds the other key");
        assert_ne!(second, first);

        provider.mark_key_success(&second);
        assert_eq!(provider.select_key().expect("sticky key"), second);

        // Single-key + empty cases.
        let solo = GeminiProvider::new("solo", "", compat(), DebugConfig::default());
        assert_eq!(solo.select_key().unwrap(), "solo");
        let empty = GeminiProvider::new("", "", compat(), DebugConfig::default());
        assert!(matches!(
            empty.select_key(),
            Err(ProviderError::MissingApiKey)
        ));
    }

    #[test]
    fn build_url_percent_encodes_hostile_model_segment() {
        // A model id containing URL structure characters must not be able to
        // inject extra path/query into the request URL.
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let url = provider.build_url("evil/../../?x=1&key=leak");
        // None of the raw structural characters survive into the segment.
        assert!(!url.contains("evil/../../"), "got: {url}");
        assert!(!url.contains("?x=1"), "got: {url}");
        // The only literal `?` is the one we put before `alt=sse`.
        assert_eq!(url.matches('?').count(), 1, "got: {url}");
        assert!(url.contains("%2F"), "slash must be percent-encoded: {url}");
        assert!(
            url.ends_with(":streamGenerateContent?alt=sse"),
            "got: {url}"
        );
    }

    // --- parse_sse_chunk tests ---

    #[test]
    fn parse_sse_chunk_emits_text_delta() {
        let mut state = GeminiStreamState::default();
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let events = parse_sse_chunk(data, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_chunk_emits_thinking_delta_for_thought_part() {
        let mut state = GeminiStreamState::default();
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"step","thought":true}]}}]}"#;
        let events = parse_sse_chunk(data, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "step"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_chunk_emits_tool_use_for_function_call() {
        let mut state = GeminiStreamState::default();
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"Read","args":{"path":"/tmp/x"}}}]}}]}"#;
        let events = parse_sse_chunk(data, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "/tmp/x");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        assert!(state.saw_tool_call);
    }

    #[test]
    fn parse_sse_chunk_captures_thought_signature_on_function_call() {
        let mut state = GeminiStreamState::default();
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"Read","args":{}},"thoughtSignature":"sig-xyz"}]}}]}"#;
        let events = parse_sse_chunk(data, &mut state);
        match &events[0] {
            LlmEvent::ToolUse { extra, .. } => {
                let extra = extra.as_ref().expect("extra must carry signature");
                assert_eq!(extra["thoughtSignature"], "sig-xyz");
            }
            other => panic!("expected ToolUse with extra, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_chunk_records_finish_reason_and_usage_on_terminal_chunk() {
        let mut state = GeminiStreamState::default();
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":3}}"#;
        parse_sse_chunk(data, &mut state);
        assert_eq!(state.final_finish_reason.as_deref(), Some("STOP"));
        assert_eq!(state.input_tokens, 12);
        assert_eq!(state.output_tokens, 3);
    }

    // --- map_gemini_finish_reason ---

    #[test]
    fn map_finish_stop_no_tools_is_end_turn() {
        let (sr, fr) = map_gemini_finish_reason("STOP", false);
        assert_eq!(sr, StopReason::EndTurn);
        assert_eq!(fr, FinishReason::Stop);
    }

    #[test]
    fn map_finish_stop_with_tools_is_tool_use() {
        let (sr, fr) = map_gemini_finish_reason("STOP", true);
        assert_eq!(sr, StopReason::ToolUse);
        assert_eq!(fr, FinishReason::Stop);
    }

    #[test]
    fn map_finish_max_tokens_is_length() {
        let (sr, fr) = map_gemini_finish_reason("MAX_TOKENS", false);
        assert_eq!(sr, StopReason::MaxTokens);
        assert_eq!(fr, FinishReason::Length);
    }

    #[test]
    fn map_finish_safety_is_error() {
        let (_sr, fr) = map_gemini_finish_reason("SAFETY", false);
        assert_eq!(fr, FinishReason::Error);
    }

    #[test]
    fn map_finish_unknown_is_error() {
        let (_sr, fr) = map_gemini_finish_reason("WHO_KNOWS", false);
        assert_eq!(fr, FinishReason::Error);
    }

    // --- F-008 sanitize_schema_for_gemini tests ----------------------------

    #[test]
    fn sanitizer_strips_additional_properties_bool() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": false
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert!(
            out.get("additionalProperties").is_none(),
            "additionalProperties:false must be stripped"
        );
        assert_eq!(out["type"], "object");
        assert_eq!(out["properties"]["x"]["type"], "string");
    }

    #[test]
    fn sanitizer_strips_additional_properties_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": { "type": "string" }
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert!(
            out.get("additionalProperties").is_none(),
            "additionalProperties object form must also be stripped"
        );
    }

    #[test]
    fn sanitizer_collapses_union_type_to_first_non_null() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "data": { "type": ["string", "array"] },
                "opt":  { "type": ["string", "null"] }
            }
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert_eq!(
            out["properties"]["data"]["type"], "string",
            "union [string, array] → string"
        );
        assert_eq!(
            out["properties"]["opt"]["type"], "string",
            "nullable union [string, null] → string"
        );
    }

    #[test]
    fn sanitizer_recurses_into_nested_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": { "val": { "type": "string" } },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert!(out.get("additionalProperties").is_none());
        assert!(
            out["properties"]["nested"]
                .get("additionalProperties")
                .is_none()
        );
    }

    #[test]
    fn sanitizer_preserves_non_hostile_keywords() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "email": {
                    "type": "string",
                    "description": "user email",
                    "format": "email"
                }
            },
            "required": ["email"]
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert_eq!(out["properties"]["email"]["type"], "string");
        assert_eq!(out["properties"]["email"]["description"], "user email");
        assert_eq!(out["properties"]["email"]["format"], "email");
        assert_eq!(out["required"], serde_json::json!(["email"]));
    }

    #[test]
    fn build_function_declarations_sanitizes_tool_schemas() {
        // Simulate a tool that emits additionalProperties:false (like pdf_tool,
        // email_parse_tool, osv_check, image_inspect_tool, google_meet_tool).
        let tools = vec![ToolDef {
            name: "pdf_extract".into(),
            description: "Extract text from a PDF".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "pages": { "type": ["integer", "null"] }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            deferred: false,
        }];
        let decls = build_function_declarations(&tools);
        assert_eq!(decls.len(), 1);
        let params = &decls[0]["parameters"];
        assert!(
            params.get("additionalProperties").is_none(),
            "additionalProperties must be stripped from Gemini function declarations"
        );
        assert_eq!(
            params["properties"]["pages"]["type"], "integer",
            "union type [integer, null] must be collapsed to 'integer'"
        );
    }
}
