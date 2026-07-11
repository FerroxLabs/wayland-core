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
use crate::tool_name::{decode_tool_name, encode_tool_name};
use crate::{
    LlmProvider, ModelInfo, ProviderError, alias_models, dump_request_body, dump_response_chunk,
    reset_response_dump,
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

    // #112: when the engine flagged this turn omit-safe (user omitted the cap
    // + model unknown to the registry + provider tolerates the absent field),
    // skip `maxOutputTokens` so Gemini applies the served model's own ceiling.
    // Belt-and-braces: also gated on THIS provider's own compat so a request
    // built against another provider's compat can never strip the field.
    let mut generation_config = if request.omit_max_tokens && compat.omit_max_tokens_when_unsized()
    {
        json!({})
    } else {
        json!({
            "maxOutputTokens": request.max_tokens,
        })
    };

    // Crucible #3: emit an explicit `temperature` when set, gated by the
    // provider's `supports_temperature` flag + the per-model exclusion. Gemini
    // nests sampling controls under `generationConfig` (not the root body), so
    // this can't reuse `openai_compat::emit_temperature` directly; the gate is
    // replicated here to match it (no hardcoded provider quirks — AGENTS.md).
    if let Some(t) = request.temperature
        && compat.supports_temperature()
        && crate::openai_compat::accepts_temperature(&request.model)
    {
        generation_config["temperature"] = json!(t);
    }

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
                            "name": encode_tool_name(name),
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
                ContentBlock::Image { mime, data } => {
                    // Gemini native shape: `parts:[{inlineData:{mimeType,data}}]`.
                    parts.push(json!({
                        "inlineData": {
                            "mimeType": mime,
                            "data": data,
                        }
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
    // Layer E1 (token-opt): serialize in a deterministic order — sorted by
    // tool name — so the functionDeclarations array is byte-identical across
    // round-trips of one conversation regardless of registration / curation
    // order. The array is part of the cached prompt prefix; a reordered
    // array changes the prefix bytes and silently busts prompt caching.
    // Schema / description / deferred are the DUPLICATE-NAME tiebreak: the
    // registry does not forbid duplicate registration, and a name-only
    // (stable) sort would keep input order for equal names — byte-unstable
    // again.
    let mut ordered: Vec<&ToolDef> = tools.iter().collect();
    ordered.sort_by_cached_key(|t| {
        (
            t.name.clone(),
            serde_json::to_string(&t.input_schema).unwrap_or_default(),
            t.description.clone(),
            t.deferred,
        )
    });
    ordered
        .iter()
        .map(|t| {
            if t.deferred {
                let short_desc = truncate_deferred_description(&t.description);
                json!({
                    "name": encode_tool_name(&t.name),
                    // Layer D2: no per-stub "use ToolSearch" boilerplate —
                    // the system prompt states the hydration rule once.
                    "description": format!("(Deferred) {short_desc}"),
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                })
            } else {
                json!({
                    "name": encode_tool_name(&t.name),
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
            // F-008c: Gemini rejects an `array` schema that omits `items`
            // (OpenAI/Anthropic are lenient). A tool whose schema omits it 400s
            // the ENTIRE request at tool registration, making Gemini unusable
            // with that toolset. Inject a permissive default so any
            // array-without-items tool still registers and works on Gemini.
            if out.get("type").and_then(Value::as_str) == Some("array")
                && !out.contains_key("items")
            {
                out.insert("items".to_string(), serde_json::json!({ "type": "string" }));
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
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|e| format!("<body read failed: {e}>"));
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

    fn alias_key(&self) -> &str {
        "gemini"
    }

    /// Live model discovery via the Generative Language API `GET
    /// /v1beta/models` endpoint. On any HTTP/parse failure we fall back to the
    /// static alias catalog — `/model` must never hard-fail.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/v1beta/models", self.base_url.trim_end_matches('/'));
        // H-2 / secrets-26: the API key rides in the `x-goog-api-key` header,
        // NOT the `?key=` query string, so it cannot leak into a URL-bearing
        // error, the `[retry]` trace, or across a 302. `build_headers` is the
        // single place that auth shape lives. A malformed/absent credential
        // can't produce a header — fall back to the alias catalog.
        let headers = match self.select_key().and_then(|key| self.build_headers(&key)) {
            Ok(h) => h,
            Err(_) => return Ok(alias_models(self.alias_key())),
        };

        let live = async {
            // FIX 3: bound the request so a hung models endpoint cannot freeze
            // the `/model` picker indefinitely (the streaming client carries no
            // request-level wall-clock cap by design).
            let response = self
                .client
                .get(&url)
                .timeout(crate::http_client::LIST_MODELS_TIMEOUT)
                .headers(headers)
                .send()
                .await?;
            if !response.status().is_success() {
                anyhow::bail!("models endpoint returned HTTP {}", response.status());
            }
            let body = response.text().await?;
            parse_gemini_models(&body)
        }
        .await;

        match live {
            Ok(models) if !models.is_empty() => Ok(models),
            _ => Ok(alias_models(self.alias_key())),
        }
    }
}

/// Parse a Generative Language API `GET /v1beta/models` response body into
/// [`ModelInfo`]s. The documented shape is
/// `{"models":[{"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
/// "supportedGenerationMethods":["generateContent", ...]}]}`.
///
/// - The `models/` prefix is stripped from `name` to form the id.
/// - `displayName` is the label when present, otherwise the label mirrors the
///   stripped id.
/// - Only entries whose `supportedGenerationMethods` includes
///   `generateContent` are kept — this drops embedding-only and other
///   non-chat models that the `/model` picker cannot use.
fn parse_gemini_models(body: &str) -> anyhow::Result<Vec<ModelInfo>> {
    let json: Value = serde_json::from_str(body)?;
    let models = json
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("models response missing `models` array"))?;
    let parsed = models
        .iter()
        .filter(|entry| {
            entry
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|m| m.as_str() == Some("generateContent"))
                })
        })
        .filter_map(|entry| {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())?;
            let id = name.strip_prefix("models/").unwrap_or(name);
            if id.is_empty() {
                return None;
            }
            let display = entry
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .unwrap_or(id);
            Some(ModelInfo {
                id: id.to_string(),
                display: display.to_string(),
            })
        })
        .collect();
    Ok(parsed)
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
    // Decode the byte stream incrementally so a multi-byte codepoint split
    // across TCP chunks is not corrupted into U+FFFD (text and tool-arg JSON).
    let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();
    // E-H3 / D4: track whether an in-band Error frame was forwarded. The
    // terminal `Done` is emitted after the loop from `final_finish_reason`;
    // a stream that closes with neither was truncated.
    let mut error_seen = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = utf8.push(&chunk);
        buffer.push_str(&text);

        // M4: cap the buffer so a delimiter-less stream cannot exhaust memory.
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "SSE frame exceeded {MAX_SSE_BUFFER_BYTES} bytes without a blank-line delimiter"
            )));
        }

        // Drain every complete frame now buffered, forwarding the events each
        // produced. SSE frame boundaries and the `data:` lines within them are
        // resolved by `drain_complete_frames` (see its doc for the CRLF-vs-LF
        // delimiter handling — current Gemini models use CRLF, older ones LF).
        for data in drain_complete_frames(&mut buffer) {
            dump_response_chunk(debug, &data);
            let events = parse_sse_chunk(&data, &mut state);
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

/// Locate the earliest SSE frame boundary (a blank line) in `buffer`.
///
/// Returns `(start_byte_offset, delimiter_len)` where `start_byte_offset` is
/// the byte index of the first delimiter byte and `delimiter_len` is how many
/// bytes the delimiter spans (so the next frame begins at
/// `start_byte_offset + delimiter_len`).
///
/// SSE frames are separated by a blank line. Servers differ on line endings:
/// current Gemini models (gemini-2.5-flash, gemini-flash-latest) use CRLF, so
/// the boundary is `\r\n\r\n` (4 bytes); older models used LF, giving `\n\n`
/// (2 bytes). We scan for whichever boundary appears first and report its
/// span. Both delimiters are ASCII, so byte offsets are valid `str` indices.
fn find_frame_boundary(buffer: &str) -> Option<(usize, usize)> {
    let crlf = buffer.find("\r\n\r\n").map(|i| (i, 4usize));
    let lf = buffer.find("\n\n").map(|i| (i, 2usize));
    match (crlf, lf) {
        (Some(c), Some(l)) => Some(if c.0 <= l.0 { c } else { l }),
        (Some(c), None) => Some(c),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    }
}

/// Split every complete SSE frame off the front of `buffer` and return the
/// `data:` payloads they carry, in order. Incomplete trailing bytes (a partial
/// frame not yet terminated by a blank line) are left in `buffer` for the next
/// chunk to complete.
///
/// A frame may carry multiple lines; only `data: …` lines yield payloads
/// (`event:`/`id:`/`:`-comment lines are skipped, per SSE). `str::lines`
/// strips the trailing `\r` of a CRLF line ending, so a `data: {...}\r` line
/// inside a CRLF-framed stream still produces the bare JSON payload.
fn drain_complete_frames(buffer: &mut String) -> Vec<String> {
    let mut payloads = Vec::new();
    while let Some((end, delim_len)) = find_frame_boundary(buffer) {
        let frame = buffer[..end].to_string();
        *buffer = buffer[end + delim_len..].to_string();
        for line in frame.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                payloads.push(data.to_string());
            }
        }
    }
    payloads
}

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
                // Decode the wire name back to the canonical tool id so the
                // call resolves against the registry (mirrors the encode in
                // `build_function_declarations` / assistant-history replay).
                let name = decode_tool_name(call.get("name").and_then(Value::as_str).unwrap_or(""));
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
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
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
    fn build_contents_user_image_becomes_inline_data() {
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text { text: "Hi".into() },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "QUJD".into(),
                },
            ],
        )];
        let (_sys, contents) = build_contents(&messages, &compat());
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["text"], "Hi");
        // Gemini native inline image shape.
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "QUJD");
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

    /// #112: when the engine flags `omit_max_tokens` AND the provider's own
    /// compat is omit-safe (the gemini preset), the body carries NO
    /// `generationConfig.maxOutputTokens` — Gemini applies the served model's
    /// own ceiling. The rest of generationConfig still assembles normally.
    #[test]
    fn build_request_body_omits_max_output_tokens_when_flagged() {
        let provider = GeminiProvider::new(
            "k",
            DEFAULT_GEMINI_BASE_URL,
            ProviderCompat::gemini_defaults(),
            DebugConfig::default(),
        );
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        request.omit_max_tokens = true;
        let body = provider.build_request_body(&request);
        assert!(
            body["generationConfig"].get("maxOutputTokens").is_none(),
            "omit_max_tokens must drop generationConfig.maxOutputTokens"
        );
    }

    /// #112 belt-and-braces: with a compat that is NOT omit-safe, the request
    /// flag alone must not strip the field — the sized value is still sent.
    #[test]
    fn build_request_body_keeps_max_output_tokens_when_compat_not_omit_safe() {
        let provider = GeminiProvider::new(
            "k",
            DEFAULT_GEMINI_BASE_URL,
            compat(), // test compat: omit_max_tokens_when_unsized unset (off)
            DebugConfig::default(),
        );
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        request.omit_max_tokens = true;
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["generationConfig"]["maxOutputTokens"], 1024,
            "a non-omit-safe compat must keep sending the sized value"
        );
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

    // --- Crucible #3: generationConfig.temperature ---

    #[test]
    fn build_request_body_emits_temperature_under_generation_config() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        request.temperature = Some(0.6);
        let body = provider.build_request_body(&request);
        // f32 0.6 widens to f64 ~0.60000002, so compare with tolerance, not ==.
        let temp = body["generationConfig"]["temperature"]
            .as_f64()
            .expect("Gemini must emit temperature under generationConfig");
        assert!(
            (temp - 0.6).abs() < 1e-6,
            "Gemini generationConfig.temperature should be ~0.6, got {temp}"
        );
    }

    #[test]
    fn build_request_body_omits_temperature_when_unset() {
        let provider = GeminiProvider::new("k", "", compat(), DebugConfig::default());
        let request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        let body = provider.build_request_body(&request);
        assert!(
            body["generationConfig"].get("temperature").is_none(),
            "no temperature must emit no field"
        );
    }

    #[test]
    fn build_request_body_omits_temperature_when_compat_opts_out() {
        let opt_out = ProviderCompat {
            supports_temperature: Some(false),
            ..compat()
        };
        let provider = GeminiProvider::new("k", "", opt_out, DebugConfig::default());
        let mut request = make_request_with_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        request.temperature = Some(0.6);
        let body = provider.build_request_body(&request);
        assert!(
            body["generationConfig"].get("temperature").is_none(),
            "supports_temperature=false must suppress the field"
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
            server: None,
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
    fn sanitizer_injects_items_for_array_missing_items() {
        // F-008c regression: Gemini 400s an `array` schema with no `items`.
        // The sanitizer must inject a default so the request still registers.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "children": { "type": "array", "description": "blocks" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        });
        let out = sanitize_schema_for_gemini(&schema);
        assert_eq!(
            out["properties"]["children"]["items"]["type"], "string",
            "array without items gets a default items injected"
        );
        assert_eq!(
            out["properties"]["tags"]["items"]["type"], "string",
            "array that already declares items is left unchanged"
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
            server: None,
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

    /// Layer E1 regression guard: the serialized functionDeclarations array
    /// must be byte-identical across two consecutive round-trips of one
    /// conversation — even when the input ToolDef order differs
    /// (registration vs curation order). The array is part of the cached
    /// prompt prefix; any byte drift silently busts prompt caching.
    #[test]
    fn tools_array_byte_stable_across_roundtrips() {
        let read = ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
            server: None,
        };
        let bash = ToolDef {
            name: "Bash".into(),
            description: "Run a shell command".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
            deferred: false,
            server: None,
        };
        let spawn = ToolDef {
            name: "SpawnTool".into(),
            description: "Spawn sub-agents".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
            deferred: true,
            server: None,
        };

        // Two builds from the same input (turn N and turn N+1).
        let defs = vec![read.clone(), bash.clone(), spawn.clone()];
        let turn1 = serde_json::to_string(&build_function_declarations(&defs)).unwrap();
        let turn2 = serde_json::to_string(&build_function_declarations(&defs)).unwrap();
        assert_eq!(turn1, turn2, "same input must serialize byte-identically");

        // A build from a reordered input (e.g. a curation pass shuffled the
        // registry order mid-conversation) must STILL be byte-identical.
        let reordered =
            serde_json::to_string(&build_function_declarations(&[spawn, bash, read])).unwrap();
        assert_eq!(
            turn1, reordered,
            "reordered input must serialize byte-identically (deterministic name sort)"
        );

        // DUPLICATE names must not reintroduce input-order dependence: the
        // registry does not forbid duplicate registration, and a stable
        // name-only sort keeps input order for equal names. The
        // schema/description tiebreak makes duplicates order-independent too.
        let dup_a = ToolDef {
            name: "Read".into(),
            description: "Read a file (duplicate registration)".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"offset": {"type": "integer"}}}),
            deferred: false,
            server: None,
        };
        let dup_b = ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
            server: None,
        };
        let one = serde_json::to_string(&build_function_declarations(&[
            dup_a.clone(),
            dup_b.clone(),
        ]))
        .unwrap();
        let other = serde_json::to_string(&build_function_declarations(&[dup_b, dup_a])).unwrap();
        assert_eq!(
            one, other,
            "duplicate names must serialize byte-identically regardless of input order"
        );
    }

    // --- tool-name sanitization (Gemini mirror of the shared codec) --------

    /// Gemini requires `functionDeclarations[N].name` to match
    /// `^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$` and 400s the whole request otherwise,
    /// so a single MCP tool with a `:`/`.` in its name aborted every turn.
    /// `build_function_declarations` now emits a wire-legal encoded name.
    #[test]
    fn build_function_declarations_sanitizes_invalid_names() {
        let tools = vec![
            ToolDef {
                name: "Browser::execute".into(),
                description: "full".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "com.microsoft-markitdown".into(),
                description: "deferred".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: true,
                server: None,
            },
        ];
        let decls = build_function_declarations(&tools);
        for (i, orig) in ["Browser::execute", "com.microsoft-markitdown"]
            .into_iter()
            .enumerate()
        {
            let wire = decls[i]["name"].as_str().unwrap();
            assert_ne!(wire, orig, "invalid name must be encoded: {wire}");
            assert!(is_gemini_name_legal(wire), "not Gemini-legal: {wire}");
        }
    }

    /// A charset-clean but over-length MCP name is clamped to a ≤ 64-char
    /// Gemini-legal name (Gemini's limit is also 64).
    #[test]
    fn build_function_declarations_clamps_over_length_names() {
        let long =
            "mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels";
        let tools = vec![ToolDef {
            name: long.into(),
            description: "test".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            deferred: false,
            server: None,
        }];
        let decls = build_function_declarations(&tools);
        let wire = decls[0]["name"].as_str().unwrap();
        assert!(is_gemini_name_legal(wire), "not Gemini-legal: {wire}");
        assert!(wire.len() <= 64, "name must be clamped to ≤64: {wire}");
    }

    /// End-to-end: the encoded name emitted by `build_function_declarations`
    /// decodes back to the canonical id when the model calls the tool (parsed
    /// via `parse_sse_chunk`), for both the charset and length regimes.
    #[test]
    fn gemini_tool_name_round_trips_through_build_and_parse() {
        for orig in [
            "Browser::execute",
            "mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels",
        ] {
            let tools = vec![ToolDef {
                name: orig.into(),
                description: "t".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            }];
            let wire = build_function_declarations(&tools)[0]["name"]
                .as_str()
                .unwrap()
                .to_string();
            let mut state = GeminiStreamState::default();
            let data = json!({
                "candidates": [{
                    "content": {"parts": [{"functionCall": {"name": wire, "args": {}}}]}
                }]
            })
            .to_string();
            let events = parse_sse_chunk(&data, &mut state);
            let called = events
                .iter()
                .find_map(|e| match e {
                    LlmEvent::ToolUse { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .expect("a ToolUse event");
            assert_eq!(called, orig, "round-trip failed for {orig}");
        }
    }

    fn is_gemini_name_legal(s: &str) -> bool {
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first.is_ascii_alphabetic() || first == '_')
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    }

    // --- SSE frame delimiter / stream-completion (CRLF regression) ----------

    /// Mirror `process_sse_stream`'s buffered split + terminal logic over a
    /// captured byte stream, WITHOUT a `reqwest::Response`. Feeds `raw` in
    /// arbitrary slices to also exercise mid-delimiter chunk boundaries.
    /// Returns the forwarded events plus the terminal outcome the real
    /// function would derive from `final_finish_reason` / `error_seen`.
    fn drive_stream(
        raw: &[u8],
        chunk_size: usize,
    ) -> (Vec<LlmEvent>, Result<Option<LlmEvent>, ()>) {
        let mut state = GeminiStreamState::default();
        let mut buffer = String::new();
        let mut events: Vec<LlmEvent> = Vec::new();
        let mut error_seen = false;

        let text = std::str::from_utf8(raw).expect("fixture is valid utf8");
        let mut rest = text;
        while !rest.is_empty() {
            let take = chunk_size.min(rest.len());
            // Avoid splitting a multi-byte char; fixtures here are ASCII so
            // `take` is always a char boundary, but guard anyway.
            let take = (take..=rest.len())
                .find(|&n| rest.is_char_boundary(n))
                .unwrap_or(rest.len());
            buffer.push_str(&rest[..take]);
            rest = &rest[take..];
            for data in drain_complete_frames(&mut buffer) {
                for event in parse_sse_chunk(&data, &mut state) {
                    if matches!(event, LlmEvent::Error(_)) {
                        error_seen = true;
                    }
                    events.push(event);
                }
            }
        }

        // Terminal logic, identical to `process_sse_stream`'s tail.
        if let Some(raw) = state.final_finish_reason.take() {
            let (stop_reason, finish_reason) = map_gemini_finish_reason(&raw, state.saw_tool_call);
            let done = LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: state.cache_read_tokens,
                },
            };
            return (events, Ok(Some(done)));
        }
        if !error_seen {
            return (events, Err(())); // truncation error
        }
        (events, Ok(None))
    }

    #[test]
    fn find_frame_boundary_matches_crlf_and_lf_and_earliest() {
        // CRLF blank line is a 4-byte boundary; LF is 2-byte.
        assert_eq!(find_frame_boundary("data: a\r\n\r\nrest"), Some((7, 4)));
        assert_eq!(find_frame_boundary("data: a\n\nrest"), Some((7, 2)));
        // No blank line yet → no boundary (partial frame stays buffered).
        assert_eq!(find_frame_boundary("data: a\r\nstill-going"), None);
        // Earliest boundary wins when both forms appear.
        let (idx, len) = find_frame_boundary("x\n\ny\r\n\r\nz").unwrap();
        assert_eq!((idx, len), (1, 2));
    }

    #[test]
    fn drain_complete_frames_leaves_partial_trailing_frame_buffered() {
        let mut buffer = "data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\r\n\r\ndata: {\"c\":".to_string();
        let payloads = drain_complete_frames(&mut buffer);
        assert_eq!(payloads, vec!["{\"a\":1}", "{\"b\":2}"]);
        // The incomplete third frame is retained for the next chunk.
        assert_eq!(buffer, "data: {\"c\":");
    }

    /// Captured verbatim from the live `gemini-2.5-flash` API
    /// (`:streamGenerateContent?alt=sse`) with a tool declared and a simple
    /// user message. Two CRLF-delimited frames: a text frame, then a terminal
    /// frame carrying `finishReason: "STOP"` + `usageMetadata`. Real model
    /// emits the blank-line separator as `\r\n\r\n`, which is exactly what the
    /// old `find("\n\n")` split missed.
    const GEMINI_25_FLASH_CRLF_SSE: &str = "data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Four.\"}],\"role\": \"model\"},\"index\": 0}],\"usageMetadata\": {\"promptTokenCount\": 51,\"candidatesTokenCount\": 2,\"totalTokenCount\": 53},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"ag4pavQTzLrj4Q-XkKnZBA\"}\r\n\r\ndata: {\"candidates\": [{\"content\": {\"role\": \"model\"},\"finishReason\": \"STOP\",\"index\": 0}],\"usageMetadata\": {\"promptTokenCount\": 51,\"candidatesTokenCount\": 2,\"totalTokenCount\": 53},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"ag4pavQTzLrj4Q-XkKnZBA\"}\r\n\r\n";

    #[test]
    fn crlf_framed_flash_stream_completes_with_stop_and_does_not_truncate() {
        // Regression: current Gemini models frame events with `\r\n\r\n`. The
        // old split on `\n\n` never matched, leaving everything buffered and
        // raising "closed before a finishReason" on a fully-complete response.
        let (events, terminal) = drive_stream(GEMINI_25_FLASH_CRLF_SSE.as_bytes(), 4096);

        // The text delta was parsed (it would have been lost when the frame
        // never split).
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "Four.")),
            "expected the model text to be emitted, got {events:?}"
        );

        // Terminal is a clean Done(STOP) — NOT the truncation error.
        match terminal {
            Ok(Some(LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            })) => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(finish_reason, FinishReason::Stop);
                assert_eq!(usage.input_tokens, 51);
                assert_eq!(usage.output_tokens, 2);
            }
            other => panic!("expected Done(STOP), got {other:?}"),
        }
    }

    #[test]
    fn crlf_stream_completes_even_when_delimiter_is_split_across_chunks() {
        // The CRLF boundary can arrive split across TCP reads (e.g. `\r\n`
        // then `\r\n`). Tiny chunks force that; completion must still fire.
        let (events, terminal) = drive_stream(GEMINI_25_FLASH_CRLF_SSE.as_bytes(), 1);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "Four.")),
        );
        assert!(
            matches!(
                terminal,
                Ok(Some(LlmEvent::Done {
                    finish_reason: FinishReason::Stop,
                    ..
                }))
            ),
            "byte-at-a-time CRLF stream must still complete cleanly"
        );
    }

    /// Captured shape for a thinking-class tool call: a `thought: true` text
    /// frame, then a terminal frame with a `functionCall` + `thoughtSignature`
    /// and `finishReason: "STOP"`. CRLF-delimited, as the live API emits.
    const GEMINI_25_FLASH_CRLF_TOOLCALL_SSE: &str = "data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Planning the call.\",\"thought\": true}],\"role\": \"model\"},\"index\": 0}],\"usageMetadata\": {\"promptTokenCount\": 48,\"thoughtsTokenCount\": 40,\"totalTokenCount\": 88},\"modelVersion\": \"gemini-2.5-flash\"}\r\n\r\ndata: {\"candidates\": [{\"content\": {\"parts\": [{\"functionCall\": {\"name\": \"get_weather\",\"args\": {\"city\": \"Paris\"}},\"thoughtSignature\": \"sig-real\"}],\"role\": \"model\"},\"finishReason\": \"STOP\",\"index\": 0}],\"usageMetadata\": {\"promptTokenCount\": 48,\"candidatesTokenCount\": 14,\"totalTokenCount\": 102},\"modelVersion\": \"gemini-2.5-flash\"}\r\n\r\n";

    #[test]
    fn crlf_thinking_tool_call_stream_emits_thinking_tooluse_and_tool_stop() {
        let (events, terminal) = drive_stream(GEMINI_25_FLASH_CRLF_TOOLCALL_SSE.as_bytes(), 4096);

        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::ThinkingDelta(t) if t == "Planning the call.")),
            "thought part must surface as ThinkingDelta, got {events:?}"
        );
        match events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
        {
            Some(LlmEvent::ToolUse {
                name, input, extra, ..
            }) => {
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "Paris");
                assert_eq!(
                    extra.as_ref().expect("signature round-trips")["thoughtSignature"],
                    "sig-real"
                );
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        // STOP with a tool call maps to ToolUse, not EndTurn.
        match terminal {
            Ok(Some(LlmEvent::Done {
                stop_reason,
                finish_reason,
                ..
            })) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                assert_eq!(finish_reason, FinishReason::Stop);
            }
            other => panic!("expected Done(ToolUse/STOP), got {other:?}"),
        }
    }

    #[test]
    fn lf_framed_stream_still_completes_for_legacy_models() {
        // Back-compat: older models used bare `\n\n`. Must still complete.
        let lf = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\ndata: {\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1}}\n\n";
        let (events, terminal) = drive_stream(lf.as_bytes(), 4096);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "hi")),
        );
        assert!(matches!(
            terminal,
            Ok(Some(LlmEvent::Done {
                finish_reason: FinishReason::Stop,
                ..
            }))
        ));
    }

    #[test]
    fn genuinely_truncated_crlf_stream_still_errors() {
        // A real dropped stream (text frame, then nothing — no finishReason)
        // must NOT be masked. The fix only stops FALSE truncation reports.
        let truncated = "data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"par\"}],\"role\": \"model\"},\"index\": 0}]}\r\n\r\n";
        let (events, terminal) = drive_stream(truncated.as_bytes(), 4096);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "par")),
        );
        assert!(
            matches!(terminal, Err(())),
            "a stream that closes before any finishReason must still error"
        );
    }

    // --- live /model library — /v1beta/models parse -----------------------

    #[test]
    fn parse_gemini_models_strips_prefix_and_uses_display_name() {
        let body = r#"{"models":[
            {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
             "supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/gemini-2.5-flash","displayName":"Gemini 2.5 Flash",
             "supportedGenerationMethods":["generateContent"]}
        ]}"#;
        let models = parse_gemini_models(body).expect("valid body parses");
        assert_eq!(models.len(), 2);
        // `models/` prefix stripped for the id; displayName used for the label.
        assert_eq!(models[0].id, "gemini-2.5-pro");
        assert_eq!(models[0].display, "Gemini 2.5 Pro");
        assert_eq!(models[1].id, "gemini-2.5-flash");
    }

    #[test]
    fn parse_gemini_models_falls_back_to_id_when_no_display_name() {
        let body = r#"{"models":[
            {"name":"models/gemini-2.5-flash-lite",
             "supportedGenerationMethods":["generateContent"]}
        ]}"#;
        let models = parse_gemini_models(body).expect("parses");
        assert_eq!(models.len(), 1);
        // No displayName → label mirrors the stripped id.
        assert_eq!(models[0].display, "gemini-2.5-flash-lite");
    }

    #[test]
    fn parse_gemini_models_filters_out_embedding_only_models() {
        // `text-embedding-004` only supports `embedContent` — it must be
        // dropped because the `/model` picker cannot drive it.
        let body = r#"{"models":[
            {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
             "supportedGenerationMethods":["generateContent"]},
            {"name":"models/text-embedding-004","displayName":"Text Embedding 004",
             "supportedGenerationMethods":["embedContent"]},
            {"name":"models/embedding-gecko","displayName":"Gecko",
             "supportedGenerationMethods":[]}
        ]}"#;
        let models = parse_gemini_models(body).expect("parses");
        assert_eq!(models.len(), 1, "only generateContent models survive");
        assert_eq!(models[0].id, "gemini-2.5-pro");
    }

    #[test]
    fn parse_gemini_models_skips_invalid_names_and_errors_on_no_models_array() {
        let body = r#"{"models":[
            {"name":"","supportedGenerationMethods":["generateContent"]},
            {"name":"models/","supportedGenerationMethods":["generateContent"]},
            {"supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-2.5-pro","supportedGenerationMethods":["generateContent"]}
        ]}"#;
        let models = parse_gemini_models(body).expect("parses");
        // Empty name, bare `models/` (empty after strip), and missing name are
        // all skipped — only the valid entry survives.
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-2.5-pro");

        // Missing `models` array → Err so the caller uses the alias fallback.
        assert!(parse_gemini_models(r#"{"error":"nope"}"#).is_err());
        assert!(parse_gemini_models("garbage").is_err());
    }

    /// INVARIANT: a models endpoint that 500s must NOT surface an error — the
    /// provider floors to the static `gemini` alias catalog so the `/model`
    /// picker never hard-fails. We point the provider at a wiremock server that
    /// always 500s and assert the returned list equals the alias floor.
    #[tokio::test]
    async fn list_models_falls_back_to_alias_on_http_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider =
            GeminiProvider::new("test-key", &server.uri(), compat(), DebugConfig::default());
        let models = provider.list_models().await.expect("never errors");
        assert_eq!(
            models,
            alias_models("gemini"),
            "a 500 must floor to the static gemini alias catalog"
        );
        assert!(!models.is_empty(), "the gemini alias catalog is non-empty");
    }

    /// A 200 with a valid body yields the live, parsed catalog (not the alias
    /// floor) — proving the happy path is wired through `list_models`.
    #[tokio::test]
    async fn list_models_returns_live_catalog_on_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = r#"{"models":[
            {"name":"models/gemini-9.0-pro","displayName":"Gemini 9.0 Pro",
             "supportedGenerationMethods":["generateContent"]}
        ]}"#;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let provider =
            GeminiProvider::new("test-key", &server.uri(), compat(), DebugConfig::default());
        let models = provider.list_models().await.expect("never errors");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-9.0-pro");
        assert_eq!(models[0].display, "Gemini 9.0 Pro");
    }
}
