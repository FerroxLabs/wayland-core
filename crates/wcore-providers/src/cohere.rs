//! Cohere provider — native chat surface (NOT OpenAI-compatible).
//!
//! Cohere ships its own chat API at `POST /v1/chat` with `Bearer` auth.
//! The request shape uses `message` (the latest user turn), `chat_history`
//! (prior turns, roles in `USER`/`CHATBOT`), `preamble` (system prompt),
//! and `tools` (function-style). Streaming responses are SSE-style JSON
//! lines with an `event_type` discriminator:
//!
//! - `text-generation`  → `LlmEvent::TextDelta`
//! - `tool-calls-generation` → `LlmEvent::ToolUse`
//! - `stream-end` → `LlmEvent::Done` (carries `finish_reason` + `response.meta.billed_units`)
//!
//! Register via [`register_cohere_in`] against a [`ProviderRegistry`]. The id
//! is lowercased to `"cohere"`.
//!
//! v0.8.1 task U10c.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{
    ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage, ToolUseId,
};

use crate::key_rotation::{KeyPool, split_keys};
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::retry::builder_send_with_retry;
use crate::{
    LlmProvider, ProviderError, dump_request_body, dump_response_chunk, reset_response_dump,
};

/// Default Cohere base URL.
pub const COHERE_DEFAULT_BASE_URL: &str = "https://api.cohere.com/v1";

/// Cohere provider — native chat surface (own body shape + SSE event types).
pub struct CohereProvider {
    client: wcore_egress::EgressClient,
    /// Rotation pool over one-or-more API keys. A single configured key yields
    /// a one-element pool — behavior identical to the pre-rotation path. Wrapped
    /// in `Arc<Mutex<…>>` so `&self` request methods can rotate/demote keys.
    keys: Arc<Mutex<KeyPool>>,
    base_url: String,
    /// Default model used when the request's `model` field is empty. Lets
    /// the registry pin a concrete model (`command-r-plus-08-2024`) while
    /// per-call `LlmRequest::model` can still override.
    default_model: String,
    debug: DebugConfig,
}

impl CohereProvider {
    /// Construct with an explicit base URL.
    pub fn new(api_key: &str, base_url: &str, default_model: &str, debug: DebugConfig) -> Self {
        Self {
            client: crate::http_client::build(),
            keys: Arc::new(Mutex::new(KeyPool::new(split_keys(api_key)))),
            base_url: base_url.to_string(),
            default_model: default_model.to_string(),
            debug,
        }
    }

    /// Construct with Cohere's default base URL.
    pub fn with_defaults(api_key: &str, default_model: &str, debug: DebugConfig) -> Self {
        Self::new(api_key, COHERE_DEFAULT_BASE_URL, default_model, debug)
    }

    /// Select the API key to authenticate the next request. Delegates to
    /// [`KeyPool::next_key`] (prefers the last-good key, rotates round-robin on
    /// failure, skips keys in cooldown). Returns [`ProviderError::MissingApiKey`]
    /// when no key is configured or every key is cooling.
    fn select_key(&self) -> Result<String, ProviderError> {
        // F19: recover the guard on poison instead of cascade-panicking —
        // KeyPool stays valid across a prior panic, so a transient fault must
        // not become a permanent provider-family DoS.
        let mut pool = self
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pool.next_key()
            .map(str::to_string)
            .ok_or(ProviderError::MissingApiKey)
    }

    /// Promote `key` to last-good after a successful (2xx) response.
    fn mark_key_success(&self, key: &str) {
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_success(key);
    }

    /// Demote `key` for the cooldown window after an auth/rate-limit failure
    /// (401/403/429), so the next request rotates to another key.
    fn mark_key_failure(&self, key: &str) {
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_failure(key);
    }

    fn build_headers(&self, key: &str) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", key);
        let auth = HeaderValue::from_str(&bearer).map_err(|e| {
            ProviderError::Connection(format!("Invalid authorization header: {}", e))
        })?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn resolved_model(&self, req: &LlmRequest) -> String {
        if req.model.trim().is_empty() {
            self.default_model.clone()
        } else {
            req.model.clone()
        }
    }
}

#[async_trait]
impl LlmProvider for CohereProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}/chat", self.base_url);
        let model = self.resolved_model(request);
        let body = build_cohere_body(request, &model);

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

        // TODO(http-error-class): wiremock tests pending for cohere HTTP error
        // class (400/401/403/429/500). The status check is correct — tests are
        // missing. See fix/providers-http-error-class for the pattern used on
        // openai / anthropic / gemini / bedrock.
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
            if let Err(e) = process_cohere_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });
        Ok(rx)
    }
}

/// Build the Cohere `/v1/chat` request body from an `LlmRequest`.
///
/// Mapping:
/// - The LAST `Role::User` message's text becomes `message`.
/// - All prior messages become `chat_history` with `User → USER`,
///   `Assistant → CHATBOT`, `System → SYSTEM`, `Tool → TOOL`.
/// - `request.system` (string) becomes `preamble`.
/// - `request.max_tokens` becomes `max_tokens`.
/// - `request.tools` are forwarded as Cohere-style tool definitions.
/// - `stream: true` always — we own the streaming path.
pub(crate) fn build_cohere_body(req: &LlmRequest, model: &str) -> Value {
    // Split messages: locate the last user message — everything before it is
    // history, that user message's text is `message`. If no user message
    // exists, `message` falls back to an empty string and `chat_history`
    // contains everything.
    let last_user_idx = req
        .messages
        .iter()
        .rposition(|m| matches!(m.role, Role::User));

    let (history_msgs, latest_message) = match last_user_idx {
        Some(idx) => {
            let latest = flatten_message_text(&req.messages[idx]);
            let history: Vec<&Message> = req.messages.iter().take(idx).collect();
            (history, latest)
        }
        None => {
            let history: Vec<&Message> = req.messages.iter().collect();
            (history, String::new())
        }
    };

    let chat_history: Vec<Value> = history_msgs
        .into_iter()
        .map(|m| {
            json!({
                "role": cohere_role(m.role),
                "message": flatten_message_text(m),
            })
        })
        .collect();

    let mut body = json!({
        "model": model,
        "message": latest_message,
        "chat_history": chat_history,
        "stream": true,
    });

    if req.max_tokens > 0 {
        body["max_tokens"] = json!(req.max_tokens);
    }

    if !req.system.is_empty() {
        body["preamble"] = json!(req.system);
    }

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name.clone(),
                    "description": t.description.clone(),
                    "parameter_definitions": t.input_schema.clone(),
                })
            })
            .collect();
        body["tools"] = json!(tools);
    }

    body
}

fn cohere_role(role: Role) -> &'static str {
    match role {
        Role::User => "USER",
        Role::Assistant => "CHATBOT",
        Role::System => "SYSTEM",
        Role::Tool => "TOOL",
    }
}

/// Concatenate all `ContentBlock::Text` segments of a message into a single
/// string. Tool blocks are skipped here (they're surfaced through the
/// `tools` channel, not the chat-history text).
fn flatten_message_text(m: &Message) -> String {
    let mut out = String::new();
    for block in &m.content {
        match block {
            ContentBlock::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ContentBlock::ToolResult { content, .. } => {
                // Surface tool results as inline text in the history — Cohere
                // doesn't have a native tool-result message role pre-`/v2`.
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(content);
            }
            ContentBlock::ToolUse { .. } | ContentBlock::Thinking { .. } => {
                // Skipped — surfaced through dedicated channels by the caller.
            }
        }
    }
    out
}

/// Maximum size a single unterminated Cohere SSE line may reach (M-20 /
/// rel-panic-67). 1 MiB is far above any legitimate Cohere stream line and
/// mirrors the cap openai / anthropic / gemini / bedrock already enforce.
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

/// Stream Cohere SSE-style JSON lines, mapping each `event_type` to an
/// `LlmEvent`. Cohere does NOT wrap events in `data: ` prefixes — each
/// line is a raw JSON object terminated by a newline.
async fn process_cohere_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    // Decode the byte stream incrementally so a multi-byte codepoint split
    // across TCP chunks is not corrupted into U+FFFD (text and tool-arg JSON).
    let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();

    let mut usage = TokenUsage::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = utf8.push(&chunk);
        buffer.push_str(&text);

        // M-20 / rel-panic-67: cap the buffer so a newline-less stream cannot
        // exhaust memory. Without this a hostile/buggy Cohere gateway that
        // streams bytes without a `\n` grows `buffer` until OOM.
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "Cohere SSE line exceeded {MAX_SSE_BUFFER_BYTES} bytes without a newline delimiter"
            )));
        }

        while let Some(line_end) = buffer.find('\n') {
            let raw = buffer[..line_end].trim().to_string();
            // M-20: `drain` instead of reslice-to-new-String avoids the O(n²)
            // copy on every consumed line.
            buffer.drain(..line_end + 1);

            if raw.is_empty() {
                continue;
            }

            // Accept both bare-JSON and `data: <json>` SSE forms — different
            // gateways front Cohere with either flavor.
            let payload = raw.strip_prefix("data: ").unwrap_or(&raw);
            if payload == "[DONE]" {
                return Ok(());
            }

            dump_response_chunk(debug, payload);

            let json: Value = match serde_json::from_str(payload) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let events = parse_cohere_event(&json, &mut usage);
            for ev in events {
                if tx.send(ev).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn parse_cohere_event(json: &Value, usage: &mut TokenUsage) -> Vec<LlmEvent> {
    let mut out = Vec::new();
    let Some(event_type) = json.get("event_type").and_then(|v| v.as_str()) else {
        return out;
    };

    match event_type {
        "text-generation" => {
            if let Some(text) = json.get("text").and_then(|v| v.as_str())
                && !text.is_empty()
            {
                out.push(LlmEvent::TextDelta(text.to_string()));
            }
        }
        "tool-calls-generation" => {
            if let Some(arr) = json.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in arr {
                    let name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = tc.get("parameters").cloned().unwrap_or(Value::Null);
                    let id: ToolUseId = tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_cohere_{}", name));
                    out.push(LlmEvent::ToolUse {
                        id,
                        name,
                        input,
                        extra: None,
                    });
                }
            }
        }
        "stream-end" => {
            // Pull usage from response.meta.billed_units (Cohere's canonical
            // field). Falls back to tokens.input_tokens etc. if present.
            if let Some(resp) = json.get("response") {
                if let Some(billed) = resp.pointer("/meta/billed_units") {
                    usage.input_tokens = billed
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    usage.output_tokens = billed
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                } else if let Some(tokens) = resp.pointer("/meta/tokens") {
                    usage.input_tokens = tokens
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    usage.output_tokens = tokens
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
            }

            let raw_finish = json
                .get("finish_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let (stop_reason, finish_reason) = map_cohere_finish(raw_finish);
            out.push(LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: usage.clone(),
            });
        }
        _ => {
            // `stream-start`, `search-queries-generation`, `tool-calls-chunk`,
            // etc. are not surfaced as LlmEvents — they're either preamble or
            // partial-tool-arg streaming we collapse into the final
            // `tool-calls-generation` event.
        }
    }

    out
}

/// Map Cohere's `finish_reason` string to (StopReason, FinishReason).
fn map_cohere_finish(s: &str) -> (StopReason, FinishReason) {
    match s {
        "COMPLETE" | "STOP_SEQUENCE" => (StopReason::EndTurn, FinishReason::Stop),
        "TOOL_CALL" => (StopReason::ToolUse, FinishReason::Stop),
        "MAX_TOKENS" => (StopReason::MaxTokens, FinishReason::Length),
        "ERROR" | "ERROR_TOXIC" | "ERROR_LIMIT" | "USER_CANCEL" => {
            (StopReason::EndTurn, FinishReason::Error)
        }
        _ => (StopReason::EndTurn, FinishReason::Error),
    }
}

/// Register a Cohere factory in the given registry under the lowercased id
/// `"cohere"`. The factory captures `api_key`, `base_url`, `default_model`,
/// and `debug` and constructs a fresh provider per call.
pub fn register_cohere_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    default_model: String,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(CohereProvider::new(
            &api_key,
            &base_url,
            &default_model,
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("cohere", factory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;
    use wcore_types::message::{ContentBlock, Message, Role};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req_three_messages_with_system() -> LlmRequest {
        LlmRequest {
            model: "command-r-plus-08-2024".into(),
            system: "you are helpful".into(),
            messages: vec![
                Message::new(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: "first".into(),
                    }],
                ),
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: "second".into(),
                    }],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: "third".into(),
                    }],
                ),
            ],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn body_extracts_last_user_into_message_and_history() {
        let req = req_three_messages_with_system();
        let body = build_cohere_body(&req, "command-r-plus-08-2024");

        assert_eq!(body["model"], "command-r-plus-08-2024");
        assert_eq!(body["message"], "third");
        assert_eq!(body["preamble"], "you are helpful");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], true);

        let history = body["chat_history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "USER");
        assert_eq!(history[0]["message"], "first");
        assert_eq!(history[1]["role"], "CHATBOT");
        assert_eq!(history[1]["message"], "second");
    }

    #[test]
    fn body_uses_empty_message_when_no_user_present() {
        let req = LlmRequest {
            model: "command-r".into(),
            system: "sys".into(),
            messages: vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            max_tokens: 16,
            ..Default::default()
        };
        let body = build_cohere_body(&req, "command-r");
        assert_eq!(body["message"], "");
        assert_eq!(body["chat_history"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn finish_reason_maps_known_values() {
        assert_eq!(
            map_cohere_finish("COMPLETE"),
            (StopReason::EndTurn, FinishReason::Stop)
        );
        assert_eq!(
            map_cohere_finish("MAX_TOKENS"),
            (StopReason::MaxTokens, FinishReason::Length)
        );
        assert_eq!(
            map_cohere_finish("TOOL_CALL"),
            (StopReason::ToolUse, FinishReason::Stop)
        );
        assert_eq!(
            map_cohere_finish("ERROR"),
            (StopReason::EndTurn, FinishReason::Error)
        );
        assert_eq!(
            map_cohere_finish("WAT"),
            (StopReason::EndTurn, FinishReason::Error)
        );
    }

    #[tokio::test]
    async fn streams_text_then_done_from_mock_server() {
        let server = MockServer::start().await;
        let sse_body = r#"{"event_type":"text-generation","text":"hello "}
{"event_type":"text-generation","text":"world"}
{"event_type":"stream-end","finish_reason":"COMPLETE","response":{"meta":{"billed_units":{"input_tokens":7,"output_tokens":3}}}}
"#;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let p = CohereProvider::new(
            "sk-test",
            &server.uri(),
            "command-r-plus-08-2024",
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "command-r-plus-08-2024".into(),
            system: String::new(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            max_tokens: 16,
            ..Default::default()
        };

        let mut rx = p.stream(&req).await.expect("stream ok");
        let mut text = String::new();
        let mut got_done = false;
        let mut final_usage_in = 0u64;
        let mut final_usage_out = 0u64;

        while let Some(ev) = rx.recv().await {
            match ev {
                LlmEvent::TextDelta(t) => text.push_str(&t),
                LlmEvent::Done { usage, .. } => {
                    got_done = true;
                    final_usage_in = usage.input_tokens;
                    final_usage_out = usage.output_tokens;
                    break;
                }
                LlmEvent::Error(e) => panic!("unexpected error: {e}"),
                _ => {}
            }
        }
        assert_eq!(text, "hello world");
        assert!(got_done, "expected Done event");
        assert_eq!(final_usage_in, 7);
        assert_eq!(final_usage_out, 3);
    }

    #[tokio::test]
    async fn stream_returns_api_error_on_4xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"message":"bad key"}"#))
            .mount(&server)
            .await;

        let p = CohereProvider::new(
            "sk-test",
            &server.uri(),
            "command-r",
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "command-r".into(),
            max_tokens: 16,
            ..Default::default()
        };
        let err = p.stream(&req).await.unwrap_err();
        match err {
            ProviderError::Api { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Api(401) got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_returns_rate_limited_on_429() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(429).set_body_string(""))
            .mount(&server)
            .await;

        let p = CohereProvider::new(
            "sk-test",
            &server.uri(),
            "command-r",
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "command-r".into(),
            max_tokens: 16,
            ..Default::default()
        };
        let err = p.stream(&req).await.unwrap_err();
        assert!(matches!(err, ProviderError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        let p = CohereProvider::new(
            "sk-test",
            "http://127.0.0.1:1",
            "command-r",
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "command-r".into(),
            max_tokens: 16,
            ..Default::default()
        };
        let result = p.stream(&req).await;
        assert!(result.is_err(), "expected error from unreachable host");
    }

    /// M-20 / rel-panic-67: a Cohere stream that never sends a newline must
    /// fail with `ProviderError::Parse` once the 1 MiB cap is exceeded,
    /// rather than buffering without bound. The error reaches the channel as
    /// `LlmEvent::Error` (the spawn maps the `Err` return).
    #[tokio::test]
    async fn stream_caps_unterminated_buffer_with_parse_error() {
        let server = MockServer::start().await;
        // 2 MiB of a single JSON-ish line with NO `\n` anywhere.
        let huge = format!("{{\"x\":\"{}\"", "a".repeat(2 * 1024 * 1024));
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(huge)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let p = CohereProvider::new(
            "sk-test",
            &server.uri(),
            "command-r",
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "command-r".into(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            max_tokens: 16,
            ..Default::default()
        };

        let mut rx = p.stream(&req).await.expect("stream starts");
        let mut saw_error = false;
        while let Some(ev) = rx.recv().await {
            if let LlmEvent::Error(msg) = ev {
                assert!(
                    msg.contains("exceeded") && msg.contains("without a newline"),
                    "expected the buffer-cap Parse error, got: {msg}"
                );
                saw_error = true;
                break;
            }
        }
        assert!(saw_error, "expected an LlmEvent::Error from the buffer cap");
    }

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_cohere_in(
            &mut r,
            "sk-test".into(),
            COHERE_DEFAULT_BASE_URL.into(),
            "command-r-plus-08-2024".into(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("cohere").is_some());
        assert!(r.get("Cohere").is_none());
        assert!(r.get("COHERE").is_none());
    }
}
