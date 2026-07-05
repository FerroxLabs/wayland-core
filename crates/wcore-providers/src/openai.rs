use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::tool::{ToolDef, truncate_deferred_description};

use crate::key_rotation::{KeyPool, split_keys};
use crate::openai_compat;
use crate::openai_responses;
use crate::retry::builder_send_with_retry;
use crate::tool_name::{decode_tool_name, encode_tool_name};
use crate::{
    LlmProvider, ModelInfo, ProviderError, alias_models, dump_request_body, dump_response_chunk,
    reset_response_dump,
};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

/// An async source of a fresh bearer token, resolved once per request. Returns
/// the raw token string to place in `Authorization: Bearer …`. Used by OAuth
/// providers (e.g. xAI/Grok) whose access token must be refreshed near expiry
/// — the closure owns the refresh round-trip, so the provider always sends a
/// live credential without the engine snapshotting a token that goes stale.
/// Distinct from a static API key: when set it fully replaces the key pool.
pub type AsyncTokenSource = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, ProviderError>> + Send>> + Send + Sync,
>;

pub struct OpenAIProvider {
    client: wcore_egress::EgressClient,
    /// Rotation pool over one-or-more API keys. A single configured key yields
    /// a one-element pool — behavior identical to the pre-rotation path. Every
    /// OpenAI-compatible newtype (Groq, DeepSeek, Together, Ollama, …) delegates
    /// here, so this seam covers the whole family at once. Wrapped in
    /// `Arc<Mutex<…>>` so `&self` request methods can rotate/demote keys.
    keys: Arc<Mutex<KeyPool>>,
    /// When set, the per-request credential comes from this async source
    /// (OAuth, refreshed near expiry) instead of the static `keys` pool. The
    /// pool is empty in that case and `select_key` is never consulted.
    bearer: Option<AsyncTokenSource>,
    base_url: String,
    /// Once a `compat.auth_fallback_base_url` retry authenticates (region-locked
    /// key failover), the working host is pinned here so every later request
    /// tries it first. `None` until a fallback has succeeded. Mirrors the same
    /// field on `AnthropicProvider`.
    pinned_base_url: Arc<Mutex<Option<String>>>,
    compat: ProviderCompat,
    debug: DebugConfig,
    /// Per-model "accepts a `tools` array" cache — the capability-first tools
    /// gate (#389/#97 follow-up). Populated by the Ollama `/api/show` probe and
    /// by reactive tools-unsupported 400s; read by `build_request_body` before
    /// attaching `body["tools"]`. Shared across clones via an inner `Arc`.
    tool_support: crate::tool_capability::ToolSupportCache,
}

impl OpenAIProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            client: crate::http_client::build(),
            keys: Arc::new(Mutex::new(KeyPool::new(split_keys(api_key)))),
            bearer: None,
            base_url: base_url.to_string(),
            pinned_base_url: Arc::new(Mutex::new(None)),
            compat,
            debug,
            tool_support: crate::tool_capability::ToolSupportCache::new(),
        }
    }

    /// Build over an async OAuth bearer source instead of a static API key.
    /// Every request resolves (and, if near expiry, refreshes) the token via
    /// `bearer` before sending — so an OAuth session never dies mid-turn on a
    /// stale snapshot. The key pool is empty; `select_key` is never used.
    pub fn with_bearer(
        bearer: AsyncTokenSource,
        base_url: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self {
            client: crate::http_client::build(),
            keys: Arc::new(Mutex::new(KeyPool::new(Vec::new()))),
            bearer: Some(bearer),
            base_url: base_url.to_string(),
            pinned_base_url: Arc::new(Mutex::new(None)),
            compat,
            debug,
            tool_support: crate::tool_capability::ToolSupportCache::new(),
        }
    }

    /// Capability-first tools gate for local Ollama backends (#389/#97
    /// follow-up). On the first turn for an Ollama-served model, probe
    /// `/api/show` and cache whether it advertises `tools` support, so
    /// [`OpenAIProvider::build_request_body`] can drop an unsupported `tools`
    /// array pre-emptively instead of eating a reactive 400. No-op for
    /// non-Ollama providers and for models already cached (by a prior probe or
    /// a reactive failure). Best-effort: a failed probe leaves the model
    /// unknown, so tools stay attached optimistically.
    async fn maybe_probe_ollama_tools(&self, model: &str) {
        if self.compat.provider_type() != "ollama" {
            return;
        }
        if self.tool_support.get(model).is_some() {
            return;
        }
        if let Some(supports) =
            crate::ollama_probe::probe_ollama_tool_support(&self.client, &self.base_url, model)
                .await
        {
            self.tool_support.set(model, supports);
        }
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
        if let Some(key) = pool.next_key() {
            return Ok(key.to_string());
        }
        // No key configured. A self-hosted / local inference endpoint (Ollama,
        // llama.cpp, LM Studio) needs none, so send a benign placeholder and let
        // the turn succeed instead of failing with `MissingApiKey` (surfaced as
        // "OpenAI API key is required") for a model the user is running locally.
        // A public host still errors, preserving the clear missing-key signal for
        // real cloud providers.
        if is_self_hosted_base_url(&self.base_url) {
            tracing::debug!(
                base_url = %self.base_url,
                "no API key for a self-hosted endpoint; using keyless placeholder bearer"
            );
            return Ok(SELF_HOSTED_PLACEHOLDER_KEY.to_string());
        }
        Err(ProviderError::MissingApiKey)
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

    /// Build request headers authenticating with the supplied `key` via the
    /// `Authorization: Bearer …` header.
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

    /// #282 contract V1 — append the Core→Flux context-aware-routing request
    /// headers to `headers`, but ONLY when this turn targets a Flux **tier
    /// alias** (`flux-auto`/`flux-fast`/`flux-standard`/`flux-reasoning`). A
    /// concrete model id opts OUT (the customer pinned the upstream), so non-Flux
    /// and concrete-model requests are left byte-for-byte unchanged.
    ///
    /// `OpenAIProvider` is shared by every OpenAI-compatible provider; the
    /// `is_flux_tier_alias` gate is the one place that quirk lives, so the
    /// headers can never leak onto a non-Flux deployment.
    ///
    /// Emits (all `x-wl-*`):
    /// - `x-wl-context-tokens`  — assembled-prompt token estimate (skipped if absent)
    /// - `x-wl-expected-output` — the output budget (`request.max_tokens`)
    /// - `x-wl-context-managed` — literal `true` (opts into the managed path)
    /// - `x-wl-conversation-id` — stable conversation id (skipped if absent)
    fn apply_flux_context_headers(headers: &mut HeaderMap, request: &LlmRequest) {
        if !is_flux_tier_alias(&request.model) {
            return;
        }
        if let Some(tokens) = request.client_context_tokens
            && let Ok(v) = HeaderValue::from_str(&tokens.to_string())
        {
            headers.insert(HeaderName::from_static("x-wl-context-tokens"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&request.max_tokens.to_string()) {
            headers.insert(HeaderName::from_static("x-wl-expected-output"), v);
        }
        headers.insert(
            HeaderName::from_static("x-wl-context-managed"),
            HeaderValue::from_static("true"),
        );
        if let Some(id) = request.conversation_id.as_deref()
            && let Ok(v) = HeaderValue::from_str(id)
        {
            headers.insert(HeaderName::from_static("x-wl-conversation-id"), v);
        }
    }

    /// #417 — resolve the per-request message compat from the target model. The
    /// strict-reasoner "must replay reasoning_content" contract is a per-MODEL
    /// requirement, but a router provider's static compat has the flag off, so a
    /// DeepSeek/Kimi turn routed through Flux/OpenRouter would drop the history's
    /// reasoning_content and 400 (wayland#417). When the target model requires
    /// replay, force the flag on for this request only; otherwise return the base
    /// compat unchanged, so a non-strict model (e.g. claude-via-Flux) never
    /// replays an unsigned thinking block. Direct DeepSeek/Kimi already set the
    /// flag, so this is a no-op clone for them.
    fn message_compat(compat: &ProviderCompat, model: &str) -> ProviderCompat {
        if openai_compat::requires_reasoning_content_replay(model)
            && !compat.replays_thinking_in_history()
        {
            let mut c = compat.clone();
            c.replays_thinking_in_history = Some(true);
            c
        } else {
            compat.clone()
        }
    }

    fn build_messages(messages: &[Message], system: &str, compat: &ProviderCompat) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

        // Replay historical assistant `reasoning_content` ONLY for the strict
        // reasoner endpoints (DeepSeek Reasoner, Moonshot/Kimi) that 400 the
        // request unless every assistant message carries reasoning_content once
        // any turn produced thinking. For every other OpenAI-family provider this
        // flag is false, so historical thinking is dropped at the wire — it is
        // billed as fresh input each turn but the model does not need it, so
        // re-sending it is pure recurring cost (finding #174). This matches the
        // Anthropic/Bedrock/Vertex adapters, which drop historical thinking.
        //
        // NOTE: this is the Chat Completions path. The Responses API path
        // (`openai_responses.rs`) drops ALL reasoning items unconditionally
        // because there they are protocol-linked to encrypted ids we do not
        // persist; that path is intentionally left untouched.
        let replay_thinking = compat.replays_thinking_in_history()
            && messages.iter().any(|m| {
                m.role == Role::Assistant
                    && m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Thinking { .. }))
            });

        // System message first
        if !system.is_empty() {
            result.push(json!({
                "role": "system",
                "content": system
            }));
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    // Check if this contains tool results
                    let has_tool_results = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result becomes a separate "tool" role message
                        for block in &msg.content {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                            {
                                result.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": content
                                }));
                            }
                        }
                    } else {
                        let text: String = msg
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        let mut text = strip_patterns_from_text(&text, compat);

                        // #648: vision capability gate. If the turn carries inline
                        // images, OpenAI Chat requires the multi-part array shape
                        // with `image_url` parts. But text-only endpoints (deepseek,
                        // cerebras, perplexity, …) 400 on an `image_url` part, so
                        // only emit it when `compat.supports_vision()`. Otherwise
                        // drop the image and append the shared placeholder text —
                        // the same soft degradation cohere / bedrock (mistral) /
                        // wayland-ollama already do.
                        let images: Vec<Value> = if compat.supports_vision() {
                            msg.content
                                .iter()
                                .filter_map(|b| {
                                    if let ContentBlock::Image { mime, data } = b {
                                        Some(json!({
                                            "type": "image_url",
                                            "image_url": {
                                                "url": format!("data:{mime};base64,{data}")
                                            }
                                        }))
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        } else {
                            if msg
                                .content
                                .iter()
                                .any(|b| matches!(b, ContentBlock::Image { .. }))
                            {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str("[image omitted: model not vision-capable]");
                            }
                            Vec::new()
                        };

                        if images.is_empty() {
                            result.push(json!({
                                "role": "user",
                                "content": text
                            }));
                        } else {
                            let mut parts: Vec<Value> = Vec::new();
                            if !text.is_empty() {
                                parts.push(json!({ "type": "text", "text": text }));
                            }
                            parts.extend(images);
                            result.push(json!({
                                "role": "user",
                                "content": parts
                            }));
                        }
                    }
                }
                Role::Assistant => {
                    let mut msg_json = json!({ "role": "assistant" });

                    // Preserve reasoning_content ONLY for the strict reasoner
                    // endpoints (DeepSeek Reasoner, Kimi) that require ALL
                    // assistant messages to include reasoning_content once any
                    // message in the conversation has it. For every other
                    // provider `replay_thinking` is false, so historical thinking
                    // is dropped here and never re-billed as input (finding #174).
                    if replay_thinking {
                        let thinking: String = msg
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Thinking { thinking } = b {
                                    Some(thinking.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        msg_json["reasoning_content"] = json!(thinking);
                    }

                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let text = strip_patterns_from_text(&text, compat);

                    let tool_calls: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                extra,
                            } = b
                            {
                                let mut tc_json = json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": encode_tool_name(name),
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
                                    }
                                });
                                // `extra_content` is an internal-only field
                                // (e.g. Gemini's google.thought routing marker
                                // captured inbound). Only echo it outbound for
                                // providers that emitted it and tolerate the
                                // round-trip; strict OpenAI-compat endpoints
                                // (Fireworks / GLM-5 via Flux) 400 on it during
                                // long-context replay (wayland-core#120).
                                if compat.emit_tool_call_extra_content()
                                    && let Some(extra_val) = extra
                                {
                                    tc_json["extra_content"] = extra_val.clone();
                                }
                                Some(tc_json)
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !text.is_empty() {
                        msg_json["content"] = json!(text);
                    } else if tool_calls.is_empty() {
                        msg_json["content"] = json!("");
                    }

                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = json!(tool_calls);
                    }

                    result.push(msg_json);
                }
                Role::System => {
                    // Already handled above
                }
                Role::Tool => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content
                            }));
                        }
                    }
                }
            }
        }

        // Dedup tool results: keep last occurrence of each tool_call_id
        if compat.dedup_tool_results() {
            dedup_tool_results(&mut result);
        }

        // #170 defense-in-depth: an empty/missing tool_call id is invalid on
        // every strict OpenAI endpoint (DeepSeek 400s the whole request on a
        // null `tool_call_id`). This is NOT gated by a compat flag — an empty id
        // is never valid anywhere — and is a no-op when all ids are present.
        strip_empty_tool_call_ids(&mut result);

        // Clean orphans in BOTH directions. OpenAI-format APIs 400 on either a
        // `tool` result with no parent `tool_calls` entry OR an assistant
        // `tool_calls` entry with no answering result. Strip results-without-a-
        // call first (e.g. left behind when history trimming drops the parent
        // assistant message — FerroxLabs/wayland#85), then calls-without-a-
        // result. Order is independent (an orphan result has no parent call, so
        // removing it never orphans a call), but results-first keeps the two
        // passes from re-scanning each other's removals.
        if compat.clean_orphan_tool_calls() {
            clean_orphaned_tool_results(&mut result);
            clean_orphaned_tool_calls(&mut result);
        }

        // Merge consecutive assistant messages
        if compat.merge_assistant_messages() {
            merge_consecutive_assistant(&mut result);
        }

        // An assistant message must carry `content` or `tool_calls`. The orphan
        // and empty-id passes above can strip the last tool_call from a
        // tool-call-only (text-less) assistant turn — leaving `{"role":
        // "assistant"}`, which native DeepSeek 400s ("content or tool_calls
        // must be set"). Stamp an empty-string content in that case so the
        // request stays valid (FerroxLabs/wayland-core#123).
        ensure_assistant_content_present(&mut result);

        result
    }

    fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                if t.deferred {
                    let short_desc = truncate_deferred_description(&t.description);
                    json!({
                        "type": "function",
                        "function": {
                            "name": encode_tool_name(&t.name),
                            "description": format!(
                                "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                            ),
                            "parameters": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    })
                } else {
                    json!({
                        "type": "function",
                        "function": {
                            "name": encode_tool_name(&t.name),
                            "description": t.description,
                            "parameters": normalize_tool_parameters(&t.input_schema)
                        }
                    })
                }
            })
            .collect()
    }

    /// Derive the `/models` listing URL from the configured chat endpoint.
    ///
    /// The chat surface is `base_url + api_path()` where `api_path()` is
    /// `/v1/chat/completions` by default (some catalog entries override it).
    /// The models endpoint sits at the same API root: strip a trailing
    /// `/chat/completions` from the path and append `/models`. When the path
    /// has no such suffix (an unusual override) fall back to the canonical
    /// `/v1/models` under the base URL so the request is still well-formed.
    fn models_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = self.compat.api_path();
        match path.strip_suffix("/chat/completions") {
            Some(root) => format!("{base}{root}/models"),
            None => format!("{base}/v1/models"),
        }
    }

    /// Derive the Responses endpoint URL (`/v1/responses`) from the configured
    /// chat endpoint, the same way [`Self::models_url`] derives `/v1/models`.
    ///
    /// The chat surface is `base_url + api_path()` where `api_path()` defaults
    /// to `/v1/chat/completions`. Strip a trailing `/chat/completions` from the
    /// path and append `/responses` so the Responses request shares the `/v1`
    /// API root. When the path has no such suffix (an unusual override) fall
    /// back to the canonical `/v1/responses` under the base URL.
    /// The host to try first: a fallback that previously authenticated this
    /// session (region-locked-key failover) if pinned, else the configured
    /// primary `base_url`.
    fn effective_base_url(&self) -> String {
        self.pinned_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| self.base_url.clone())
    }

    /// Remember the host that authenticated so later requests skip the primary's
    /// certain 401.
    fn pin_base_url(&self, url: &str) {
        *self
            .pinned_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(url.to_string());
    }

    /// Build the request URL for a specific `base_url` (chat-completions or, for
    /// the gpt-5 family, the Responses API). Separated from `self.base_url` so
    /// the region-failover path can target an alternate host.
    fn url_for(&self, base_url: &str, use_responses: bool) -> String {
        if use_responses {
            self.responses_url_for(base_url)
        } else {
            format!("{}{}", base_url, self.compat.api_path())
        }
    }

    /// Send one streaming request to a specific `base_url` and return the 2xx
    /// response, or the mapped [`ProviderError`]. Region failover (retrying an
    /// alternate host on a 401/403) is the caller's concern — this targets
    /// exactly the host it is given. `key` is resolved once by the caller and
    /// reused across hosts (a region-locked key is valid on exactly one).
    async fn try_send(
        &self,
        base_url: &str,
        use_responses: bool,
        body: &Value,
        key: &str,
        using_bearer: bool,
        request: &LlmRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = self.url_for(base_url, use_responses);
        // #282: attach the Core→Flux context-routing headers (gated to Flux
        // tier aliases inside the helper); non-Flux requests are unchanged.
        let mut headers = self.build_headers(key)?;
        Self::apply_flux_context_headers(&mut headers, request);
        let response =
            builder_send_with_retry(self.client.post(&url).headers(headers).json(body)).await?;

        let status = response.status();
        if !status.is_success() {
            // Demote this key on auth / rate-limit failures so the next request
            // rotates to another key in the pool (no-op for a single key, and
            // skipped for the OAuth bearer path which holds one credential).
            if !using_bearer && matches!(status.as_u16(), 401 | 403 | 429) {
                self.mark_key_failure(key);
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
            // FluxRouter folds its paid-only gating into the OpenAI-compatible
            // 402 surface. Map the recognised codes to typed entitlement errors
            // so the CLI can message a feature lock vs an account-needs-payment
            // state distinctly; unrecognised 402s fall through to `Api`.
            if status.as_u16() == 402
                && let Some(err) = parse_flux_402(&body_text)
            {
                return Err(err);
            }
            // #282 contract V1: a managed Flux client that overflows the routed
            // model's window gets a typed context-overflow error. Surface it as
            // `ProviderError::ContextOverflow` so the engine can compact-then-retry
            // this same turn. Live Flux has shipped this as BOTH 409
            // (`context_overflow`) and 413 (`context_window_exceeded`) across
            // versions, so accept either status; an unrecognised body still falls
            // through to the generic `Api` error.
            if matches!(status.as_u16(), 409 | 413)
                && let Some(err) = parse_flux_overflow(&body_text)
            {
                return Err(err);
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        // 2xx: this key works — make it sticky for subsequent requests.
        if !using_bearer {
            self.mark_key_success(key);
        }
        Ok(response)
    }

    fn responses_url_for(&self, base_url: &str) -> String {
        let base = base_url.trim_end_matches('/');
        let path = self.compat.api_path();
        match path.strip_suffix("/chat/completions") {
            Some(root) => format!("{base}{root}/responses"),
            None => format!("{base}/v1/responses"),
        }
    }

    /// True when this request must be served via the OpenAI Responses API
    /// instead of Chat Completions. Per-request (not per-provider): the same
    /// `OpenAIProvider` serves `gpt-4o` + `gpt-5` in one session. Honors the
    /// optional `ProviderCompat.uses_responses_api` override before falling
    /// back to the model-family default. See `openai_compat` module docs.
    fn uses_responses_api(&self, request: &LlmRequest) -> bool {
        openai_compat::responses_api_override(&request.model, self.compat.uses_responses_api())
    }

    pub(crate) fn build_request_body(&self, request: &LlmRequest) -> Value {
        // Per-request detection: the same OpenAIProvider instance serves
        // multiple models in one session (e.g. gpt-4o + gpt-5), so the
        // family-specific request shape MUST be decided per request, not
        // baked into `self.compat`. See `openai_compat` module docs.
        //
        // The `self.compat.max_tokens_field` override still wins for
        // non-OpenAI deployments that need a custom field name; the
        // detector only overrides it when the request targets a model
        // family that REQUIRES `max_completion_tokens`.
        let max_tokens_field = if openai_compat::max_completion_tokens_override(
            &request.model,
            self.compat.uses_max_completion_tokens(),
        ) {
            "max_completion_tokens"
        } else {
            self.compat
                .max_tokens_field
                .as_deref()
                .unwrap_or("max_tokens")
        };

        // #417 — resolve the effective message compat for THIS request's target
        // model, so a router (Flux/OpenRouter) replays reasoning_content when it
        // routes to a strict reasoner (DeepSeek/Kimi) without 400ing, while a
        // non-strict model keeps replay off. See `message_compat`.
        let msg_compat = Self::message_compat(&self.compat, &request.model);
        let mut body = json!({
            "model": request.model,
            "messages": Self::build_messages(&request.messages, &request.system, &msg_compat),
            "stream": true
        });
        // `stream_options: {include_usage: true}` asks for token accounting in
        // the final chunk. Default on, but suppressible for generic self-hosted
        // OpenAI-compatible endpoints that 400 on the unknown field (#86).
        if self.compat.include_usage_in_stream() {
            body["stream_options"] = json!({ "include_usage": true });
        }
        // #112: when the engine flagged this turn omit-safe (user omitted the
        // cap + model unknown to the registry + provider tolerates the absent
        // field), skip the wire field so the served model's natural output
        // ceiling applies. `request.max_tokens` still carries the sized
        // internal budget for the x-wl-expected-output header below.
        // Belt-and-braces: the request flag is ALSO gated on THIS provider's
        // own compat, so a request built against another provider's compat
        // (or a future direct caller) can never strip the field from an
        // endpoint that requires a sized value.
        if !(request.omit_max_tokens && self.compat.omit_max_tokens_when_unsized()) {
            body[max_tokens_field] = json!(request.max_tokens);
        }

        // FluxRouter web_search grounding (contract §5.2 / §5.8). Grounding only
        // fires when the model is a tier alias (the customer let Flux pick) AND
        // no real function tools ride along (Sonar rejects tools — function tools
        // SUPPRESS grounding). When the caller asked for `web_search` on a tier
        // alias, prefer grounding semantics for the turn: emit ONLY the
        // `{"type":"web_search"}` tool and drop any function tools. A concrete
        // model id (or `web_search` unset) keeps the normal function-tool path —
        // injecting the tool there would not ground and would only confuse the
        // concrete model, so we skip it.
        //
        // On the normal function-tool path, also gate on the model family:
        // Groq's agentic Compound models reject a caller-supplied `tools` array
        // with a 400 that kills the turn (they do their own internal tool use).
        // Per-request, since one provider serves many models in a session —
        // mirrors the `reasoning_effort` gate below.
        let ground_web_search = request.web_search && is_flux_tier_alias(&request.model);
        if ground_web_search {
            body["tools"] = json!([{ "type": "web_search" }]);
        } else if !request.tools.is_empty()
            && openai_compat::model_supports_tool_calling(&request.model)
            && self.tool_support.allows(&request.model)
        {
            body["tools"] = json!(Self::build_tools(&request.tools));
        }

        // Flux sticky-session / prefix-cache key. On a Flux tier alias the
        // conversation id doubles as the OpenAI `prompt_cache_key`: the router
        // uses it to pin the whole conversation to one backend (a mid-session
        // backend hop is a cold cache pool, re-billing the full prefix at full
        // price), and OpenAI-compatible upstreams use it as their documented
        // cache-routing hint. Gated exactly like the x-wl-* context headers: a
        // concrete model id (or an absent conversation_id) keeps the body
        // byte-identical to today, since generic OpenAI-compatible endpoints
        // can 400 on unknown fields.
        if is_flux_tier_alias(&request.model)
            && let Some(id) = request.conversation_id.as_deref()
            && !id.trim().is_empty()
        {
            body["prompt_cache_key"] = json!(id);
        }

        // Gate `reasoning_effort` on the model family. gpt-4o (and other
        // classic chat families) 400 on the field; only o1*/o3*/gpt-5*
        // accept it. This MUST stay per-request: one OpenAIProvider serves
        // many models in a session, so `self.compat.supports_effort` (a
        // provider-level flag) cannot decide it. R78 dedups the predicate —
        // `accepts_reasoning_effort` now forwards to the single canonical copy
        // in `wcore-config`.
        if let Some(effort) = &request.reasoning_effort
            && openai_compat::accepts_reasoning_effort(&request.model)
        {
            body["reasoning_effort"] = json!(effort);
        }

        // Crucible #3: emit an explicit `temperature` when set, gated by the
        // provider's `supports_temperature` flag + the per-model o-series
        // exclusion (see `openai_compat::emit_temperature`).
        openai_compat::emit_temperature(&mut body, request, &self.compat);

        // Output-side optimization (Part A): UNION the request's fluff stop
        // sequences into OpenAI's `stop` field, preserving any the caller
        // already placed on the body. The Chat Completions API accepts `stop`
        // as a string or an array of up to 4 strings; normalize a pre-existing
        // string into an array before appending. Skipped when empty so
        // back-compatible callers emit no stop field.
        if !request.stop_sequences.is_empty() {
            match body.get_mut("stop") {
                Some(existing) if existing.is_array() => {
                    let arr = existing.as_array_mut().expect("checked is_array");
                    for s in &request.stop_sequences {
                        arr.push(json!(s));
                    }
                }
                Some(existing) if existing.is_string() => {
                    let mut arr = vec![existing.clone()];
                    for s in &request.stop_sequences {
                        arr.push(json!(s));
                    }
                    body["stop"] = json!(arr);
                }
                _ => {
                    body["stop"] = json!(request.stop_sequences);
                }
            }
        }

        body
    }
}

/// A benign bearer for a self-hosted endpoint configured without an API key.
/// Local inference servers (Ollama, llama.cpp, LM Studio, vLLM) ignore the
/// `Authorization` header, so any non-empty value works. Sending this lets a
/// keyless local connection succeed instead of failing the turn with
/// `MissingApiKey` (surfaced in the UI as "OpenAI API key is required").
const SELF_HOSTED_PLACEHOLDER_KEY: &str = "wayland-local";

/// True when `base_url`'s host is a self-hosted address that is plausibly
/// keyless: loopback (`localhost`, `127.0.0.0/8`, `::1`), unspecified
/// (`0.0.0.0`/`::`), Docker (`host.docker.internal`), mDNS (`*.local`), an
/// RFC1918 private LAN range (`10/8`, `172.16/12`, `192.168/16`), or the
/// Tailscale / CGNAT range (`100.64.0.0/10`). Public hosts return `false`, so a
/// real cloud provider with a missing key still surfaces a clear `MissingApiKey`
/// rather than silently sending a bogus bearer and getting a 401.
fn is_self_hosted_base_url(base_url: &str) -> bool {
    // Host = strip scheme, take up to the first '/', drop any `user@`, strip the
    // `:port`. IPv6 literals are bracketed (`[::1]:11434`).
    let after_scheme = base_url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(base_url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    }
    .trim()
    .to_ascii_lowercase();

    if host.is_empty() {
        return false;
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host == "host.docker.internal"
        || host.ends_with(".local")
        || host == "0.0.0.0"
        || host == "::1"
        || host == "::"
    {
        return true;
    }
    // IPv4 loopback / private / CGNAT ranges. Every dotted segment must parse as
    // a u8, so a hostname like `api.openai.com` (a non-numeric segment) yields an
    // empty vec and falls through to `false`.
    let octets: Vec<u8> = host
        .split('.')
        .map(|o| o.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();
    if octets.len() == 4 {
        return matches!(
            (octets[0], octets[1]),
            (127, _) | (10, _) | (192, 168) | (172, 16..=31) | (100, 64..=127)
        );
    }
    false
}

/// True when a provider HTTP error means "this model does not support tool
/// calling" — the request was otherwise valid and would succeed if the `tools`
/// array were dropped.
///
/// Local OpenAI-compatible backends without tool support reject the request
/// with a 400 whose body names the missing capability. Unlike Groq's Compound
/// family — a small, known set we can name-gate in
/// [`openai_compat::model_supports_tool_calling`] — local no-tool models are
/// open-ended (any small model the user has pulled), so we cannot enumerate
/// them. Detect the error and retry once without tools so the turn still
/// completes. See FerroxLabs/wayland#389.
///
/// Backends differ in wording, so we match a set of specific markers rather
/// than one provider's phrasing:
/// - Ollama: `... does not support tools`
/// - llama.cpp without `--jinja`: `tools param requires --jinja flag`, `Unsupported param: tools`
/// - generic: `does not support function calling`, `tools are not supported`, etc.
///
/// Matched conservatively: 400-only, and each marker is specific enough not to
/// collide with unrelated 400s (a 500 is never retried — it may be a transient
/// server fault, not a stable capability gap). Gated upstream by
/// `body_has_tools`, so we only ever drop a `tools` array that was actually
/// attached.
fn is_tools_unsupported_error(status: u16, body: &str) -> bool {
    /// Phrases local backends emit when a tools/function-calling request hits a
    /// model (or server config) that cannot do tools. Lowercase; matched as
    /// case-insensitive substrings.
    const TOOLS_UNSUPPORTED_MARKERS: &[&str] = &[
        "does not support tools",            // Ollama
        "tools param requires",              // llama.cpp without --jinja
        "unsupported param: tools",          // llama.cpp (ggml-org/llama.cpp#10920)
        "does not support function calling", // generic
        "tool calling is not supported",     // generic
        "tools are not supported",           // generic
        "tool use is not supported",         // generic
    ];
    if status != 400 {
        return false;
    }
    // Match the provider's error MESSAGE, not the whole body. A raw 400 body can
    // echo the request — tool descriptions, the user's prompt — and a marker
    // appearing there would false-positive, strip tools, and (worse) poison the
    // capability cache to text-only for the rest of the session. Extract
    // `error.message` when the body is structured JSON; fall back to the raw
    // body for non-JSON error surfaces.
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.to_owned())
        .to_ascii_lowercase();
    TOOLS_UNSUPPORTED_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
}

/// Strip configured patterns from text content
fn strip_patterns_from_text(text: &str, compat: &ProviderCompat) -> String {
    match &compat.strip_patterns {
        Some(patterns) if !patterns.is_empty() => {
            let mut result = text.to_string();
            for pattern in patterns {
                result = result.replace(pattern, "");
            }
            result
        }
        _ => text.to_string(),
    }
}

/// Normalize a tool's input schema into a `function.parameters` object that
/// strict OpenAI-compatible servers accept.
///
/// Lenient backends (OpenAI, most clouds) tolerate a bare `{"type":"object"}`,
/// but strict local servers such as LM Studio reject any `function.parameters`
/// that lacks a `properties` object (HTTP 400). Built-in tools and MCP-provided
/// tools that declare no structured arguments emit exactly that bare schema, so
/// guarantee the three fields the JSON-Schema function-calling contract wants:
/// `type: "object"`, a `properties` object, and a `required` array. Existing
/// fields (real `properties`, `additionalProperties`, etc.) are preserved.
/// See FerroxLabs/wayland#24.
fn normalize_tool_parameters(schema: &Value) -> Value {
    let mut obj = match schema {
        Value::Object(map) => map.clone(),
        // null / non-object schema -> the empty object schema strict servers want
        _ => serde_json::Map::new(),
    };
    obj.entry("type").or_insert_with(|| json!("object"));
    if !obj.get("properties").is_some_and(Value::is_object) {
        obj.insert("properties".to_string(), json!({}));
    }
    if !obj.get("required").is_some_and(Value::is_array) {
        obj.insert("required".to_string(), json!([]));
    }
    Value::Object(obj)
}

/// Deduplicate tool results: keep last occurrence of each tool_call_id
fn dedup_tool_results(messages: &mut Vec<Value>) {
    use std::collections::HashMap;

    // Find the last index of each tool_call_id
    let mut last_index: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
        {
            last_index.insert(id.to_string(), i);
        }
    }

    // Keep only the last occurrence
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut to_remove = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
            && let Some(&last_i) = last_index.get(id)
        {
            if i != last_i && !seen.contains_key(id) {
                to_remove.push(i);
            }
            if i == last_i {
                seen.insert(id.to_string(), true);
            }
        }
    }

    // Remove in reverse order to preserve indices
    for i in to_remove.into_iter().rev() {
        messages.remove(i);
    }
}

/// Remove `tool` result messages whose `tool_call_id` matches no assistant
/// `tool_calls` entry — the symmetric counterpart to
/// [`clean_orphaned_tool_calls`].
///
/// OpenAI-format APIs reject (HTTP 400) any `tool` message that does not answer
/// a preceding assistant `tool_calls` entry. Such an orphan arises when the
/// parent assistant message is dropped from the request while its tool result
/// is kept — e.g. a context-window trim that splits the pair. An orphaned tool
/// result is unconditionally invalid to send, so stripping it is strictly
/// correct. See FerroxLabs/wayland#85.
fn clean_orphaned_tool_results(messages: &mut Vec<Value>) {
    use std::collections::HashSet;

    let called_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("assistant"))
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter_map(|tc| tc["id"].as_str().map(String::from))
        .collect();

    messages.retain(|m| {
        if m["role"].as_str() == Some("tool") {
            // Keep a tool result only when it answers an assistant tool_calls
            // entry that survives in the array. A result whose tool_call_id is
            // unmatched — OR missing/non-string entirely — is an orphan that
            // strict OpenAI endpoints (DeepSeek via Flux) 400 on, so drop it
            // (FerroxLabs/wayland-core#123). The empty/missing-id case is also
            // caught upstream by `strip_empty_tool_call_ids`; handling it here
            // too keeps this pass correct in isolation, independent of call
            // order or compat gating.
            m["tool_call_id"]
                .as_str()
                .is_some_and(|id| called_ids.contains(id))
        } else {
            // Non-tool messages are out of scope for this pass.
            true
        }
    });
}

/// #170 defense-in-depth: strip any tool-call / tool-result whose id is empty
/// or missing.
///
/// A tool-role message must carry a non-empty `tool_call_id`, and an assistant
/// `tool_calls[].id` must be non-empty — strict OpenAI endpoints (DeepSeek in
/// particular) reject the WHOLE request with a 400 on a null/empty id, which
/// hard-strands the conversation. This happens when an upstream router (e.g.
/// Flux on the DeepSeek leg) drops the streamed tool-call id; the engine
/// faithfully echoes the empty value back, and the next request 400s.
///
/// Rather than send a guaranteed-400 request, drop the empty-id tool exchange
/// so the conversation degrades gracefully (it loses one tool round-trip)
/// instead of dying. A `warn!` is emitted whenever anything is stripped so the
/// upstream defect stays VISIBLE — this guard masks the symptom but must never
/// silently hide the root cause (see the Flux handoff for the real fix).
fn strip_empty_tool_call_ids(messages: &mut Vec<Value>) {
    let is_empty_id = |v: &Value| v.as_str().map(str::is_empty).unwrap_or(true);
    let mut stripped = 0usize;

    // 1. Drop empty-id entries from assistant `tool_calls`.
    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant")
            && let Some(tcs) = msg["tool_calls"].as_array_mut()
        {
            let before = tcs.len();
            tcs.retain(|tc| !is_empty_id(&tc["id"]));
            stripped += before - tcs.len();
            if tcs.is_empty() {
                // Same invariant as clean_orphaned_tool_calls: a message with a
                // `tool_calls` array is an object.
                msg.as_object_mut().unwrap().remove("tool_calls");
            }
        }
    }

    // 2. Drop tool-role messages with an empty/missing `tool_call_id`.
    let before = messages.len();
    messages.retain(|m| !(m["role"].as_str() == Some("tool") && is_empty_id(&m["tool_call_id"])));
    stripped += before - messages.len();

    if stripped > 0 {
        tracing::warn!(
            stripped,
            "stripped {stripped} tool message(s) with an empty/missing tool_call_id before \
             sending to the provider — an upstream router likely dropped the streamed \
             tool-call id (FerroxLabs/wayland#170); the request would otherwise 400",
        );
    }
}

/// Remove tool_call entries from assistant messages that have no corresponding tool result
fn clean_orphaned_tool_calls(messages: &mut [Value]) {
    use std::collections::HashSet;

    let answered_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("tool"))
        .filter_map(|m| m["tool_call_id"].as_str().map(String::from))
        .collect();

    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant")
            && let Some(tcs) = msg["tool_calls"].as_array_mut()
        {
            tcs.retain(|tc| {
                tc["id"]
                    .as_str()
                    .map(|id| answered_ids.contains(id))
                    .unwrap_or(true)
            });
            if tcs.is_empty() {
                // SAFETY: the outer match arm verified that msg's
                // `tool_calls` is an array, which means msg is itself
                // a JSON object (only objects can have keyed
                // children). `as_object_mut` is therefore always Some.
                msg.as_object_mut().unwrap().remove("tool_calls");
            }
        }
    }
}

/// Guarantee every assistant message carries `content` or `tool_calls`.
///
/// `build_messages` omits `content` for a tool-call-only assistant turn (its
/// text is empty), and the orphan/empty-id cleanup passes can then strip that
/// turn's only `tool_calls` entry — leaving `{"role":"assistant"}` with
/// neither field. Strict OpenAI endpoints reject it: native DeepSeek returns
/// HTTP 400 "Invalid assistant message: content or tool_calls must be set".
/// Stamping an empty-string content (the same value `build_messages` uses for
/// a genuinely empty assistant turn) keeps the request valid without inventing
/// text (FerroxLabs/wayland-core#123).
fn ensure_assistant_content_present(messages: &mut [Value]) {
    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant")
            && msg.get("tool_calls").is_none()
            && !msg["content"].is_string()
            && let Some(obj) = msg.as_object_mut()
        {
            obj.insert("content".to_string(), json!(""));
        }
    }
}

/// Merge consecutive assistant messages into one
fn merge_consecutive_assistant(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i + 1 < messages.len() {
        if messages[i]["role"].as_str() == Some("assistant")
            && messages[i + 1]["role"].as_str() == Some("assistant")
        {
            let next = messages.remove(i + 1);

            // Merge text content
            let curr_text = messages[i]["content"].as_str().unwrap_or("").to_string();
            let next_text = next["content"].as_str().unwrap_or("").to_string();
            let merged_text = match (curr_text.is_empty(), next_text.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_text,
                (false, true) => curr_text,
                (false, false) => format!("{}{}", curr_text, next_text),
            };

            if !merged_text.is_empty() {
                messages[i]["content"] = json!(merged_text);
            }

            // Merge reasoning_content
            let curr_rc = messages[i]["reasoning_content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let next_rc = next["reasoning_content"].as_str().unwrap_or("").to_string();
            let merged_rc = match (curr_rc.is_empty(), next_rc.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_rc,
                (false, true) => curr_rc,
                (false, false) => format!("{}{}", curr_rc, next_rc),
            };

            if !merged_rc.is_empty() {
                messages[i]["reasoning_content"] = json!(merged_rc);
            }

            // Merge tool_calls
            if let Some(next_tcs) = next["tool_calls"].as_array() {
                // SAFETY: messages[i] is an "assistant" message
                // verified by the outer `if` to have role==assistant,
                // which is always a JSON object.
                let curr_tcs = messages[i]
                    .as_object_mut()
                    .unwrap()
                    .entry("tool_calls")
                    .or_insert_with(|| json!([]));
                if let Some(arr) = curr_tcs.as_array_mut() {
                    arr.extend(next_tcs.iter().cloned());
                }
            }

            // Don't increment i - check the merged result against the next message
        } else {
            i += 1;
        }
    }
}

/// State for accumulating tool call deltas by index
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
    extra: Option<Value>,
}

struct StreamState {
    tool_calls: Vec<ToolCallAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
    /// Cache-read (prompt cache hit) tokens reported by the chat path's usage
    /// chunk. Informational only — `input_tokens` already includes these on the
    /// OpenAI chat surface, so this is not added to/subtracted from input.
    cache_read_tokens: u64,
    /// Deferred Done event: populated when finish_reason arrives, emitted on
    /// [DONE] so the final usage-only chunk has a chance to update token counts.
    pending_done: Option<LlmEvent>,
    /// FluxRouter web_search grounding (contract §5.4). A grounded Sonar stream
    /// carries `citations` (URL strings) and `search_results` (source cards) as
    /// TOP-LEVEL fields on the streamed frames (alongside `choices`, NOT inside
    /// `choices[].delta`). They may repeat across frames, so we accumulate +
    /// dedupe here and emit a single `Citations` / `SearchResults` event at
    /// end-of-stream. ⚠️ The EXACT streamed-frame placement is UNVERIFIED — the
    /// contract documents the non-streaming body; a live `curl -N` capture must
    /// confirm which frame(s) carry these (see `merge_flux_grounding`).
    citations: Vec<String>,
    search_results: Vec<wcore_types::llm::FluxSearchResult>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            pending_done: None,
            citations: Vec::new(),
            search_results: Vec::new(),
        }
    }

    /// FluxRouter web_search grounding (contract §5.4): merge any TOP-LEVEL
    /// `citations` / `search_results` arrays carried on a streamed frame into
    /// the accumulator, de-duplicating. Citations dedupe on the URL string;
    /// search results dedupe on `url`. Called per frame from `parse_sse_chunk`.
    ///
    /// ⚠️ UNVERIFIED frame placement: this reads the fields off the frame ROOT
    /// (`json["citations"]` / `json["search_results"]`). If a live capture shows
    /// Sonar nests them elsewhere on the streamed chunk, this is the single
    /// one-line change point — adjust the two `json.get(...)` lookups.
    fn merge_flux_grounding(&mut self, json: &Value) {
        if let Some(cites) = json.get("citations").and_then(Value::as_array) {
            for c in cites {
                if let Some(url) = c.as_str()
                    && !self.citations.iter().any(|existing| existing == url)
                {
                    self.citations.push(url.to_string());
                }
            }
        }
        if let Some(results) = json.get("search_results").and_then(Value::as_array) {
            for r in results {
                // Per-element: a malformed card is skipped, not fatal — grounding
                // is best-effort metadata, never the turn's payload.
                if let Ok(card) =
                    serde_json::from_value::<wcore_types::llm::FluxSearchResult>(r.clone())
                    && !self
                        .search_results
                        .iter()
                        .any(|existing| existing.url == card.url && !card.url.is_empty())
                {
                    self.search_results.push(card);
                }
            }
        }
    }

    /// Emit the deferred Done event with up-to-date token counts.
    ///
    /// OpenAI sends usage in a separate trailing chunk (choices:[]) *after* the
    /// chunk that carries `finish_reason`. We defer the Done event until [DONE]
    /// so that token counts are always accurate.
    fn flush_done(&mut self) -> Option<LlmEvent> {
        let pending = self.pending_done.take()?;
        Some(match pending {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                ..
            } => LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage: TokenUsage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: self.cache_read_tokens,
                },
            },
            other => other,
        })
    }

    fn get_or_create_tool(&mut self, index: usize) -> &mut ToolCallAccumulator {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCallAccumulator {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
                extra: None,
            });
        }
        &mut self.tool_calls[index]
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // Per-request API-surface routing: the `gpt-5*` family is rejected at
        // `/v1/chat/completions` and MUST use the Responses API. Everything
        // else keeps the chat path unchanged. See `uses_responses_api`.
        let use_responses = self.uses_responses_api(request);
        // Capability-first tools gate (#389/#97 follow-up): for a local Ollama
        // backend, probe `/api/show` once per model so an unsupported `tools`
        // array is dropped BEFORE the request (inside `build_request_body`)
        // rather than reactively after a 400. No-op for non-Ollama providers.
        // Only worth a probe when this turn could actually attach tools — a
        // plain chat turn has nothing to strip.
        if !request.tools.is_empty() {
            self.maybe_probe_ollama_tools(&request.model).await;
        }
        let body = if use_responses {
            openai_responses::build_responses_body(request, &self.compat)
        } else {
            self.build_request_body(request)
        };

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        // OAuth providers resolve (and refresh near expiry) a fresh bearer per
        // request; static-key providers select from the rotation pool. The key
        // is resolved ONCE and reused across the failover attempt — the same
        // credential is tried against both hosts (a region-locked key is valid
        // on exactly one).
        let using_bearer = self.bearer.is_some();
        let key = match &self.bearer {
            Some(src) => (src)().await?,
            None => self.select_key()?,
        };

        // Whether the body we are about to send actually carries a `tools`
        // array. Used to gate the tools-unsupported retry below: dropping tools
        // only helps if tools were attached in the first place.
        let body_has_tools = body.get("tools").is_some();

        let primary = self.effective_base_url();
        let response = match self
            .try_send(&primary, use_responses, &body, &key, using_bearer, request)
            .await
        {
            Ok(resp) => resp,
            // Tools-unsupported retry (#389): an Ollama model without tool
            // support rejects the request with a 400 `... does not support
            // tools`. The request is otherwise valid, so rebuild the body
            // without the `tools` array and retry once on the same host so the
            // turn completes instead of surfacing a raw provider 400. Only fires
            // when tools were actually attached.
            Err(ProviderError::Api {
                status,
                ref message,
            }) if body_has_tools && is_tools_unsupported_error(status, message) => {
                tracing::warn!(
                    model = %request.model,
                    "model does not support tools; retrying request without tools (#389)"
                );
                // Capability-first follow-up: remember this model rejects tools
                // so every later turn drops the array pre-emptively — covers
                // backends with no capability endpoint to probe (e.g. llama.cpp).
                self.tool_support.set(&request.model, false);
                let mut no_tools_request = request.clone();
                no_tools_request.tools.clear();
                no_tools_request.web_search = false;
                let no_tools_body = if use_responses {
                    openai_responses::build_responses_body(&no_tools_request, &self.compat)
                } else {
                    self.build_request_body(&no_tools_request)
                };
                self.try_send(
                    &primary,
                    use_responses,
                    &no_tools_body,
                    &key,
                    using_bearer,
                    &no_tools_request,
                )
                .await?
            }
            // Region-locked-key failover: a credential rejected here (401/403)
            // may belong to the provider's alternate platform (e.g. Moonshot's
            // `api.moonshot.cn` vs `.ai`). When a fallback host is configured
            // and we haven't already pinned it, retry the SAME key there; pin it
            // for the session on success. See `compat.auth_fallback_base_url`.
            Err(ProviderError::Api { status, .. })
                if matches!(status, 401 | 403)
                    && self
                        .compat
                        .auth_fallback_base_url
                        .as_deref()
                        .is_some_and(|fb| fb != primary) =>
            {
                let fallback = self
                    .compat
                    .auth_fallback_base_url
                    .clone()
                    .expect("is_some_and guarantees Some");
                let resp = self
                    .try_send(&fallback, use_responses, &body, &key, using_bearer, request)
                    .await?;
                self.pin_base_url(&fallback);
                resp
            }
            Err(e) => return Err(e),
        };

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        // #282 contract V1: Flux SIGNALS-BACK via `x-flux-*` response headers on
        // every response. Read them BEFORE `response` is moved into the body
        // task (and before `bytes_stream()` consumes it), then emit a single
        // `ProviderMeta` event at stream start. Non-Flux providers send none of
        // these headers, so the parse yields `None` and nothing is emitted —
        // the SSE body path is entirely unchanged.
        let provider_meta = parse_flux_response_meta(response.headers());

        tokio::spawn(async move {
            if let Some(meta) = provider_meta {
                let _ = tx.send(meta).await;
            }
            let result = if use_responses {
                process_responses_sse_stream(response, &tx, &debug).await
            } else {
                process_sse_stream(response, &tx, &debug).await
            };
            if let Err(e) = result {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }

    /// FIX 4: the alias-catalog fallback key is the provider's REAL identity,
    /// not a hardcoded `"openai"`. The same `OpenAIProvider` serves the native
    /// OpenAI arm AND every openai-compatible catalog entry (DeepSeek, Together,
    /// Groq, …); `compat.provider_type()` carries the stamped catalog id
    /// (`openai_defaults()` stamps `"openai"`, `from_catalog_entry` stamps the
    /// entry id). Returning the real id means a catalog provider whose live
    /// `/v1/models` is unreachable falls back to its OWN (typically empty) alias
    /// set — an honest "couldn't fetch models" — instead of advertising OpenAI
    /// `gpt-*` ids it cannot serve (which 404 on the next turn).
    fn alias_key(&self) -> &str {
        self.compat.provider_type()
    }

    /// Live model discovery via the OpenAI-compatible `GET /v1/models`
    /// endpoint. Covers the OpenAI native arm plus the ~104-entry catalog,
    /// which all route through `OpenAIProvider`. On any HTTP/parse failure we
    /// fall back to the static alias catalog — `/model` must never hard-fail.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = self.models_url();
        let headers = match self.select_key().and_then(|key| self.build_headers(&key)) {
            Ok(h) => h,
            // A malformed/absent credential can't produce a header — fall back.
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
            parse_openai_models(&body)
        }
        .await;

        match live {
            Ok(models) if !models.is_empty() => Ok(models),
            // Empty live list or any error: the alias catalog is the floor.
            _ => Ok(alias_models(self.alias_key())),
        }
    }
}

/// Parse an OpenAI-compatible `GET /v1/models` response body into
/// [`ModelInfo`]s. The documented shape is `{"data":[{"id":"..."}, ...]}`;
/// only `id` is required (OpenAI does not return a display name, so `display`
/// mirrors `id`). Entries missing a non-empty string `id` are skipped.
fn parse_openai_models(body: &str) -> anyhow::Result<Vec<ModelInfo>> {
    let json: Value = serde_json::from_str(body)?;
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("models response missing `data` array"))?;
    let models = data
        .iter()
        .filter_map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(ModelInfo::from_id)
        })
        .collect();
    Ok(models)
}

/// Maximum size a single unterminated SSE frame is allowed to reach before
/// the parser gives up. M4: a hostile or malfunctioning endpoint that streams
/// bytes without ever emitting a `\n` would otherwise grow `buffer` without
/// bound. 1 MiB is far above any legitimate SSE frame.
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

/// Maximum accumulated size of a single tool call's streamed `arguments` JSON.
/// B2/FIX 5: `arguments` is appended across many SSE frames
/// (`acc.arguments.push_str`); unlike the raw line buffer (capped by
/// [`MAX_SSE_BUFFER_BYTES`], which clears at each `\n`), this string survives
/// across frames and would grow without bound under a malicious or runaway
/// stream. 8 MiB is far above any legitimate tool-argument payload while still
/// bounding memory; on exceed the stream errors out rather than growing.
const MAX_TOOL_ARGS_BYTES: usize = 8 * 1024 * 1024;

/// Maximum number of parallel tool-call accumulator slots on the chat path.
/// #136: `StreamState::get_or_create_tool(index)` grows `tool_calls` until it
/// is long enough to index `index`, so a runaway or hostile stream that sends
/// a huge `tool_calls[].index` (e.g. `4_000_000_000`) would allocate unbounded
/// slots — an OOM reached BEFORE the per-call [`MAX_TOOL_ARGS_BYTES`] cap can
/// apply, since no arguments have streamed yet. A real assistant turn emits a
/// handful of parallel calls; 1024 is far beyond any legitimate use. On exceed
/// the stream errors out (fail closed) rather than growing.
const MAX_TOOL_CALLS: usize = 1024;

pub(crate) async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    // Decode the byte stream incrementally so a multi-byte codepoint split
    // across TCP chunks is not corrupted into U+FFFD (text and tool-arg JSON).
    let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();
    // E-H3 / D4: track whether a terminal event (Done or in-band Error)
    // was emitted. A stream that closes without one is a truncated turn,
    // not a clean empty success — surface it as an error.
    let mut terminal_seen = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = utf8.push(&chunk);
        buffer.push_str(&text);

        // M4: cap the buffer so a delimiter-less stream cannot exhaust memory.
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "SSE frame exceeded {MAX_SSE_BUFFER_BYTES} bytes without a newline delimiter"
            )));
        }

        // Process complete lines
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                dump_response_chunk(debug, data);
                if data == "[DONE]" {
                    // FluxRouter web_search grounding (contract §5.4): emit the
                    // accumulated, deduped citations / source cards just before
                    // the terminal Done, so a consumer renders the Sources block
                    // after the answer text. Skipped when grounding never fired
                    // (both empty), so non-Flux turns are unaffected.
                    if !state.citations.is_empty() {
                        let _ = tx.send(LlmEvent::Citations(state.citations.clone())).await;
                    }
                    if !state.search_results.is_empty() {
                        let _ = tx
                            .send(LlmEvent::SearchResults(state.search_results.clone()))
                            .await;
                    }
                    // Flush the deferred Done event now that the final
                    // usage-only chunk (choices:[]) has updated token counts.
                    if let Some(done) = state.flush_done() {
                        let _ = tx.send(done).await;
                        return Ok(());
                    }
                    // D4: `[DONE]` arrived but no `finish_reason` chunk was
                    // ever seen — the stream was cut before the model
                    // finished. Treat as a truncation, not a clean turn.
                    return Err(ProviderError::Parse(
                        "OpenAI SSE stream sent [DONE] with no finish_reason — \
                         response truncated before completion"
                            .into(),
                    ));
                }

                let events = parse_sse_chunk(data, &mut state);
                for event in events {
                    // `parse_sse_chunk` defers `Done` (flushed on `[DONE]`),
                    // so the only terminal event it can return is `Error`
                    // (the in-band error frame, E-H3).
                    if matches!(event, LlmEvent::Error(_)) {
                        terminal_seen = true;
                    }
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    // E-H3 / D4: the byte stream ended. If no terminal event was emitted,
    // the connection closed before `[DONE]` / a finish_reason / an error
    // frame — a silent truncation. Returning Err makes the provider's
    // spawn forward an `LlmEvent::Error` instead of just closing the
    // channel (which the engine would mis-read as a clean empty turn).
    if !terminal_seen {
        return Err(ProviderError::Connection(
            "OpenAI SSE stream closed before any terminal event ([DONE] / \
             finish_reason / error) — response truncated"
                .into(),
        ));
    }

    Ok(())
}

/// Process the OpenAI **Responses API** SSE byte stream into [`LlmEvent`]s.
///
/// Mirrors the chunking / UTF-8 decoding / buffer-cap / truncation-detection
/// discipline of [`process_sse_stream`] (the chat path), but parses Responses
/// events via [`openai_responses::parse_responses_event`]. The Responses stream
/// has NO `[DONE]` sentinel — the terminal frame is `response.completed`
/// (mapped to [`LlmEvent::Done`]) or `response.failed` / `error` (mapped to
/// [`LlmEvent::Error`]). A byte stream that closes before any terminal event is
/// a silent truncation and surfaces as a connection error.
pub(crate) async fn process_responses_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = openai_responses::ResponsesStreamState::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();
    let mut terminal_seen = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = utf8.push(&chunk);
        buffer.push_str(&text);

        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "SSE frame exceeded {MAX_SSE_BUFFER_BYTES} bytes without a newline delimiter"
            )));
        }

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                dump_response_chunk(debug, data);
                // The Responses stream uses no `[DONE]` sentinel; the OpenAI
                // SDK still tolerates one defensively, so skip it if present.
                if data == "[DONE]" {
                    continue;
                }

                // Whether this raw frame is a stream-terminal *error* frame
                // (`response.failed` / `error`). A per-tool-call argument
                // parse error is emitted on an `output_item.done` frame and is
                // NOT terminal — the stream still ends on its own
                // `response.completed`. So we decide terminality from the
                // frame type, not from the `LlmEvent` variant (mirrors the
                // chat path, which only treats the in-band error frame as
                // terminal).
                let is_error_frame = openai_responses::is_terminal_error_frame(data);

                let events = openai_responses::parse_responses_event(data, &mut state);
                for event in events {
                    if matches!(event, LlmEvent::Done { .. }) {
                        let _ = tx.send(event).await;
                        return Ok(());
                    }
                    if is_error_frame {
                        terminal_seen = true;
                    }
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    if !terminal_seen {
        return Err(ProviderError::Connection(
            "OpenAI Responses SSE stream closed before any terminal event \
             (response.completed / response.failed / error) — response truncated"
                .into(),
        ));
    }

    Ok(())
}

fn parse_sse_chunk(data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            // L1: a malformed `data:` frame previously vanished with no
            // diagnostic. Log it (truncated) so a provider emitting subtly
            // broken frames is debuggable instead of producing a silent
            // no-Done truncation downstream.
            let preview: String = data.chars().take(200).collect();
            tracing::warn!(
                target: "wcore_providers::openai",
                error = %e,
                payload = %preview,
                "discarding malformed OpenAI SSE data frame"
            );
            return events;
        }
    };

    // E-H3: OpenAI-compatible providers can emit an in-band error frame —
    // `{"error":{"message":..,"type":..}}` — with no `choices`. Without
    // this arm `parse_sse_chunk` finds no choices and returns zero events,
    // so the stream silently ends as an empty "successful" turn. Surface
    // it as an `Error` event (matching the Anthropic parser's `error` arm).
    if let Some(err_obj) = json.get("error") {
        let msg = err_obj
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                // No `message` field — surface the whole error object so
                // the detail is not lost.
                err_obj.to_string()
            });
        events.push(LlmEvent::Error(msg));
        return events;
    }

    // FluxRouter web_search grounding (contract §5.4): accumulate any top-level
    // `citations` / `search_results` carried on THIS frame. Done before the
    // `choices` extraction so a frame that carries grounding metadata but no
    // choices (e.g. a final citations-only chunk) still contributes. The
    // accumulated set is emitted once at end-of-stream (see `process_sse_stream`).
    state.merge_flux_grounding(&json);

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        let base_prompt = usage["prompt_tokens"]
            .as_u64()
            .unwrap_or(state.input_tokens);

        // DeepSeek-style: prompt_cache_hit_tokens is reported separately and
        // prompt_tokens only contains the cache-miss portion.
        // Add it to get the true total prompt size.
        let cache_hit = usage["prompt_cache_hit_tokens"].as_u64().unwrap_or(0);

        state.input_tokens = base_prompt + cache_hit;
        state.output_tokens = usage["completion_tokens"]
            .as_u64()
            .unwrap_or(state.output_tokens);

        // Cache-read accounting. DeepSeek reports the hit count in the separate
        // `prompt_cache_hit_tokens` field (cache-miss-only prompt_tokens); the
        // OpenAI-standard surface reports it under
        // `prompt_tokens_details.cached_tokens` (prompt_tokens already total).
        // Either way the cache-read count is informational and must be surfaced
        // so chat-path sessions show cache savings, matching the Responses path.
        let cached_details = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if cache_hit > 0 || cached_details > 0 {
            state.cache_read_tokens = cache_hit.max(cached_details);
        }
    }

    let Some(choice) = json["choices"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    let delta = &choice["delta"];

    // Per-turn thinking SUBJECT (#318). Flux's Chat Completions stream emits
    // `delta.reasoning_summary` — a short opaque display string (a gerund
    // phrase like "Reasoning through the problem") exactly once per turn,
    // immediately BEFORE the first `delta.reasoning_content` chunk, only on
    // turns that produce reasoning. Surface it as a distinct subject event
    // that the host attaches as the heading of the in-flight thinking block.
    // Opaque — never switch on the value. Absent ⇒ no subject (never
    // synthesized). This mirrors the Responses-API `reasoning_summary` path
    // in `openai_responses.rs`, unified on the same thinking channel.
    if let Some(summary) = delta["reasoning_summary"].as_str()
        && !summary.is_empty()
    {
        events.push(LlmEvent::ThinkingSubject(summary.to_string()));
    }

    // Reasoning content (OpenAI reasoning models). OpenAI o-series and
    // most reasoning-capable openai-compat providers (incl. Gemini Pro
    // on the v1beta openai-compat surface) put thoughts under
    // `delta.reasoning_content`. Some Gemini-served openai-compat
    // variants drop the `_content` suffix and emit just `delta.reasoning`
    // — accept either field to avoid silently swallowing thoughts as
    // text on the affected models. (B.3 fix.)
    let reasoning_str = delta["reasoning_content"]
        .as_str()
        .or_else(|| delta["reasoning"].as_str());
    if let Some(reasoning) = reasoning_str
        && !reasoning.is_empty()
    {
        events.push(LlmEvent::ThinkingDelta(reasoning.to_string()));
    }

    // Gemini Pro reasoning marker: Gemini's openai-compat surface
    // sometimes flags reasoning chunks by setting
    // `delta.extra_content.google.thought == true` on what is otherwise
    // a normal `delta.content` text chunk. Without this branch the
    // current decoder folds those chunks into `LlmEvent::TextDelta`,
    // which is the visible symptom of the v0.1.x Gemini reasoning-token
    // bug (B.3) — thoughts leak into the final text and downstream the
    // host re-emits the resolved answer, giving the "dropped/duplicated"
    // character signature. Route them to `ThinkingDelta` so the host
    // can render the thinking channel separately.
    let is_gemini_thought = delta
        .get("extra_content")
        .and_then(|ec| ec.get("google"))
        .and_then(|g| g.get("thought"))
        .and_then(|t| t.as_bool())
        .unwrap_or(false);

    // Text content
    if let Some(content) = delta["content"].as_str()
        && !content.is_empty()
    {
        if is_gemini_thought {
            events.push(LlmEvent::ThinkingDelta(content.to_string()));
        } else {
            events.push(LlmEvent::TextDelta(content.to_string()));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            // #136: reject an out-of-range tool-call index BEFORE
            // `get_or_create_tool` grows the accumulator Vec to `index` slots.
            // Fail closed by aborting the stream, mirroring the arg-bytes cap
            // below — a stream claiming an absurd index is buggy or hostile and
            // no tool should run from it.
            if index >= MAX_TOOL_CALLS {
                events.push(LlmEvent::Error(format!(
                    "tool-call index {index} exceeds {MAX_TOOL_CALLS} — \
                     aborting stream to bound memory"
                )));
                return events;
            }
            let acc = state.get_or_create_tool(index);

            if let Some(id) = tc["id"].as_str() {
                acc.id = id.to_string();
            }
            // Only overwrite when non-empty — some third-party APIs send `"name":""`
            // in every delta chunk which would erase the real name from the first chunk.
            if let Some(name) = tc["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                acc.name = name.to_string();
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                // FIX 5: cap accumulated tool-arg JSON so a runaway/malicious
                // stream cannot grow it without bound.
                if acc.arguments.len().saturating_add(args.len()) > MAX_TOOL_ARGS_BYTES {
                    events.push(LlmEvent::Error(format!(
                        "tool-call arguments exceeded {MAX_TOOL_ARGS_BYTES} bytes — \
                         aborting stream to bound memory"
                    )));
                    return events;
                }
                acc.arguments.push_str(args);
            }
            if let Some(extra) = tc.get("extra_content").filter(|v| !v.is_null()) {
                acc.extra = Some(extra.clone());
            }
        }
    }

    // Check finish_reason — defer Done until [DONE] so the trailing usage
    // chunk (choices:[]) can update token counts first.
    if let Some(finish_reason_raw) = choice["finish_reason"].as_str() {
        let finish_reason = map_openai_finish_reason(finish_reason_raw);
        if finish_reason == FinishReason::Error {
            // "content_filter", future additions, anything else — log so the
            // engine operator can correlate against the host's stream_end.
            eprintln!(
                "[wcore-providers] openai: unrecognized finish_reason {finish_reason_raw:?}, mapping to FinishReason::Error"
            );
        }
        let stop_reason = match (finish_reason_raw, state.tool_calls.is_empty()) {
            // tool_calls / stop with pending calls → flush them as ToolUse
            ("tool_calls" | "stop", false) => {
                for tc in state.tool_calls.drain(..) {
                    // Fail closed: non-empty argument JSON that does not parse
                    // must not run the tool with empty input — emit an error and
                    // skip the call. Empty arguments remain a valid empty object.
                    let trimmed = tc.arguments.trim();
                    let input = if trimmed.is_empty() {
                        Value::Object(serde_json::Map::new())
                    } else {
                        match serde_json::from_str::<Value>(trimmed) {
                            Ok(v) => v,
                            Err(e) => {
                                events.push(LlmEvent::Error(format!(
                                    "tool-call arguments for '{}' did not parse as JSON: {e}",
                                    tc.name
                                )));
                                continue;
                            }
                        }
                    };
                    events.push(LlmEvent::ToolUse {
                        id: tc.id,
                        name: decode_tool_name(&tc.name),
                        input,
                        extra: tc.extra,
                    });
                }
                StopReason::ToolUse
            }
            ("stop", true) => StopReason::EndTurn,
            // "tool_calls" with empty accumulator — defensive; treat as ToolUse.
            ("tool_calls", true) => StopReason::ToolUse,
            ("length", _) => StopReason::MaxTokens,
            // Unmapped: keep agent loop alive with EndTurn; FinishReason::Error
            // already flags the protocol-level signal.
            _ => StopReason::EndTurn,
        };
        state.pending_done = Some(LlmEvent::Done {
            stop_reason,
            finish_reason,
            usage: TokenUsage::default(),
        });
    }

    events
}

/// Map an OpenAI-shape `finish_reason` to `FinishReason`.
///
/// The same OpenAI-compatible path also serves Gemini (via
/// `openai-compat`) and any third-party provider that follows the OpenAI
/// chat-completions wire format. `tool_calls` and `stop` both map to
/// `Stop`; `length` maps to `Length`; anything else (`content_filter`,
/// future additions) maps to `Error` so downstream UIs see the signal
/// instead of a silent `Stop`.
pub(crate) fn map_openai_finish_reason(raw: &str) -> FinishReason {
    match raw {
        "stop" | "tool_calls" => FinishReason::Stop,
        "length" => FinishReason::Length,
        _ => FinishReason::Error,
    }
}

/// True when `model` is a FluxRouter **tier alias** — the only models on which
/// `web_search` grounding fires (contract §5.2 / §5.8). A tier alias means the
/// customer let Flux pick the upstream model; Flux then reroutes a grounded
/// turn to Perplexity Sonar. A request naming a **concrete** model id (e.g.
/// `gpt-5`, `kimi-k2-6`, `claude-*`) is treated as an explicit choice and is
/// NOT rerouted, so attaching a web_search tool there would never ground.
///
/// Matched case-insensitively against the four documented aliases. This is the
/// one place that quirk lives; callers consult it rather than string-matching
/// inline.
pub fn is_flux_tier_alias(model: &str) -> bool {
    matches!(
        model.to_ascii_lowercase().as_str(),
        "flux-auto" | "flux-fast" | "flux-standard" | "flux-reasoning"
    )
}

/// Parse a FluxRouter 402 body into a typed entitlement error.
///
/// Flux folds its paid-only gating into the OpenAI-compatible 402 surface and
/// the body arrives in several shapes (contract §2 / §3.6 / §4.6 / §5.6):
///
/// - **image** `premium_locked`:
///   `{"error":{"message":"image generation requires a paid plan","code":"premium_locked"}}`
/// - **web_fetch** `upgrade_required`:
///   `{"error":"upgrade_required","message":"web_fetch is a paid capability; ..."}`
/// - **chat money-axis** `spend_ceiling_unresolved` — DOUBLY WRAPPED in the
///   LiteLLM envelope: the outer `error.message` is a *stringified* JSON whose
///   inner object is
///   `{"error":"spend_ceiling_unresolved","reason":"no_account_id","message":"...","upgrade_url":"..."}`.
///
/// Strategy: read the recognised code/reason from BOTH the outer envelope
/// (`error` may be a string code, or an object with `code`) AND, when the outer
/// `message`/`error` is itself a JSON string, the inner object. Returns `None`
/// for an unrecognised 402 so the caller falls back to [`ProviderError::Api`].
pub(crate) fn parse_flux_402(body: &str) -> Option<ProviderError> {
    let outer: Value = serde_json::from_str(body).ok()?;

    // The inner (recovered) object, if the envelope double-wraps JSON in a
    // string. Try the LiteLLM `error.message` first, then a top-level
    // `error`/`message` that happens to be a stringified JSON object.
    let inner: Option<Value> = outer
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| outer.get("error").and_then(Value::as_str))
        .or_else(|| outer.get("message").and_then(Value::as_str))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object);

    // The recognised code can live in several spots; check inner first (it is
    // the authoritative Flux body when present), then the outer envelope.
    let code = inner
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(Value::as_str)
        .or_else(|| {
            outer
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
        })
        .or_else(|| outer.get("error").and_then(Value::as_str))?;

    // Human-readable message: prefer the most specific available.
    let message = inner
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            outer
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
        })
        .or_else(|| outer.get("message").and_then(Value::as_str))
        .unwrap_or(code)
        .to_string();

    match code {
        "premium_locked" => Some(ProviderError::PremiumLocked {
            capability: "image generation".to_string(),
            message,
        }),
        "upgrade_required" => Some(ProviderError::UpgradeRequired { message }),
        "spend_ceiling_unresolved" => {
            let reason = inner
                .as_ref()
                .and_then(|v| v.get("reason"))
                .and_then(Value::as_str)
                .or_else(|| outer.get("reason").and_then(Value::as_str))
                .unwrap_or("unknown")
                .to_string();
            let upgrade_url = inner
                .as_ref()
                .and_then(|v| v.get("upgrade_url"))
                .and_then(Value::as_str)
                .or_else(|| outer.get("upgrade_url").and_then(Value::as_str))
                .map(str::to_string);
            Some(ProviderError::SpendCeilingUnresolved {
                reason,
                upgrade_url,
            })
        }
        _ => None,
    }
}

/// Parse a FluxRouter hard-context-overflow body into a typed
/// [`ProviderError::ContextOverflow`] (#282 contract V1, hard-overflow path).
///
/// Tolerant by necessity: live Flux has shipped this signal in three shapes
/// across versions, and the body shape diverged from the frozen spec. We accept
/// any of them, matching on the overflow `error` CODE (`context_overflow` or
/// `context_window_exceeded`), never the HTTP status alone:
///
/// - A. flat (frozen spec): `{"error":"context_overflow","required_tokens":N,…}`
/// - A'. FastAPI detail: `{"detail":{"error":"context_window_exceeded",…}}`
/// - B. live prod envelope: `{"error":{"message":"{'error': 'context_overflow', …}"}}`
///   — an OpenAI/LiteLLM envelope wrapping a Python dict *repr* with single quotes.
///
/// Numbers are also recovered by a digit scan so a quirky `message`/`reason`
/// value can never hide the overflow. Returns `None` when the body is not an
/// overflow signal — the caller then falls back to [`ProviderError::Api`].
pub(crate) fn parse_flux_overflow(body: &str) -> Option<ProviderError> {
    // Flux signals a hard context overflow under TWO error codes across versions:
    // `context_overflow` (shipped on 409) and `context_window_exceeded` (the
    // in-flight 413 path). Accept either; the engine treats both as compact-retry.
    fn is_overflow(s: &str) -> bool {
        s == "context_overflow" || s == "context_window_exceeded"
    }
    // Build a `ContextOverflow` from a recovered structured object. `message`
    // falls back to `reason` (the 413 shape names it `reason`); the window /
    // routed-model fields are absent on some shapes, so they default.
    fn from_obj(obj: &Value) -> ProviderError {
        ProviderError::ContextOverflow {
            required_tokens: obj
                .get("required_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            model_window: obj.get("model_window").and_then(Value::as_u64).unwrap_or(0),
            routed_model: obj
                .get("routed_model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            message: obj
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| obj.get("reason").and_then(Value::as_str))
                .unwrap_or("context overflow")
                .to_string(),
        }
    }

    let outer: Value = serde_json::from_str(body).ok()?;

    // Shape A — flat body (frozen spec): a top-level string `error` overflow code
    // plus sibling fields: {"error":"context_overflow","required_tokens":N,...}.
    if outer
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(is_overflow)
    {
        return Some(from_obj(&outer));
    }
    // Shape A' — FastAPI `HTTPException(detail={...})` raw shape, if not re-wrapped:
    //   {"detail":{"error":"context_window_exceeded","required_tokens":N,...}}
    if let Some(d) = outer.get("detail")
        && d.get("error")
            .and_then(Value::as_str)
            .is_some_and(is_overflow)
    {
        return Some(from_obj(d));
    }

    // Shape B — what live api.fluxrouter.ai ACTUALLY returns (verified 2026-06-23):
    // an OpenAI/LiteLLM error envelope wrapping the payload as a Python dict *repr*
    // (single quotes) inside `error.message`:
    //   {"error":{"message":"{'error': 'context_overflow', 'required_tokens': N, ...}",
    //             "type":"None","code":"409"}}
    // serde can't parse the single-quoted repr, so recover it by normalizing the
    // Python literals to JSON; numbers are also scanned directly so a quirky
    // message value can never hide the overflow signal.
    let inner_str = outer
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| outer.get("error").and_then(Value::as_str))
        .or_else(|| outer.get("message").and_then(Value::as_str))
        .or_else(|| outer.get("detail").and_then(Value::as_str))?;
    if !inner_str.contains("context_overflow") && !inner_str.contains("context_window_exceeded") {
        return None;
    }
    let recovered: Option<Value> = serde_json::from_str::<Value>(inner_str)
        .ok()
        .or_else(|| {
            let normalized = inner_str
                .replace('\'', "\"")
                .replace("None", "null")
                .replace("True", "true")
                .replace("False", "false");
            serde_json::from_str::<Value>(&normalized).ok()
        })
        .filter(Value::is_object);

    // Digit-scan fallback: find `key` then the first run of ASCII digits after it.
    let scan_u64 = |key: &str| -> u64 {
        inner_str
            .find(key)
            .and_then(|i| {
                inner_str[i + key.len()..]
                    .split(|c: char| !c.is_ascii_digit())
                    .find(|s| !s.is_empty())
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .unwrap_or(0)
    };
    if let Some(obj) = recovered.as_ref() {
        // Recovered cleanly — but the window/model may still be absent (413
        // shape), so backfill the numbers from the digit scan when missing.
        let mut err = from_obj(obj);
        if let ProviderError::ContextOverflow {
            required_tokens,
            model_window,
            ..
        } = &mut err
        {
            if *required_tokens == 0 {
                *required_tokens = scan_u64("required_tokens");
            }
            if *model_window == 0 {
                *model_window = scan_u64("model_window");
            }
        }
        return Some(err);
    }
    Some(ProviderError::ContextOverflow {
        required_tokens: scan_u64("required_tokens"),
        model_window: scan_u64("model_window"),
        routed_model: String::new(),
        message: "context overflow".to_string(),
    })
}

/// Parse the FluxRouter SIGNALS-BACK `x-flux-*` response headers (#282 contract
/// V1) into a single [`LlmEvent::ProviderMeta`]. Returns `None` only when NONE
/// of the four headers is present (a non-Flux provider), so the engine emits
/// nothing on those routes. Each field is parsed independently — an absent or
/// unparsable single header is `None`, never an error.
///
/// Headers:
/// - `x-flux-routed-model`           → `routed_model`     (str)
/// - `x-flux-model-window`           → `model_window`     (int)
/// - `x-flux-context-pressure`       → `context_pressure` (float, REQUIRED/window)
/// - `x-flux-context-tokens-counted` → `tokens_counted`   (int)
fn parse_flux_response_meta(headers: &HeaderMap) -> Option<LlmEvent> {
    let as_str = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let routed_model = as_str("x-flux-routed-model").map(str::to_string);
    let model_window = as_str("x-flux-model-window").and_then(|s| s.parse::<u64>().ok());
    let context_pressure = as_str("x-flux-context-pressure").and_then(|s| s.parse::<f32>().ok());
    let tokens_counted =
        as_str("x-flux-context-tokens-counted").and_then(|s| s.parse::<u64>().ok());

    // No Flux signal present at all → nothing to emit (non-Flux response).
    if routed_model.is_none()
        && model_window.is_none()
        && context_pressure.is_none()
        && tokens_counted.is_none()
    {
        return None;
    }
    Some(LlmEvent::ProviderMeta {
        routed_model,
        model_window,
        context_pressure,
        tokens_counted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::debug::DebugConfig;

    fn no_compat() -> ProviderCompat {
        ProviderCompat::default()
    }

    // --- #136: tool-call accumulator Vec bound ----------------------------

    #[test]
    fn tool_call_index_over_cap_aborts_without_unbounded_growth() {
        // A hostile/buggy stream claims an absurd tool-call index. Pre-fix,
        // `get_or_create_tool` grew `StreamState::tool_calls` to `index + 1`
        // slots (here ~1029, but a real attack sends billions → OOM). It must
        // now be rejected with an Error and leave the accumulator Vec empty.
        let mut state = StreamState::new();
        let data = format!(
            r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":{},"id":"call_x","function":{{"name":"f","arguments":"{{}}"}}}}]}}}}]}}"#,
            MAX_TOOL_CALLS + 5
        );
        let events = parse_sse_chunk(&data, &mut state);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LlmEvent::Error(m) if m.contains("exceeds"))),
            "an out-of-range tool-call index must surface an Error, got: {events:?}"
        );
        assert!(
            state.tool_calls.is_empty(),
            "no accumulator slots may be allocated for an out-of-range index"
        );
    }

    #[test]
    fn tool_call_index_within_cap_accumulates_normally() {
        // Guard must not be too tight: a legitimate small index still
        // accumulates. Index 3 creates slots 0..=3 and records the tool.
        let mut state = StreamState::new();
        let data = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":3,"id":"call_a","function":{"name":"lookup","arguments":"{\"q\":1}"}}]}}]}"#;
        let events = parse_sse_chunk(data, &mut state);
        assert!(
            !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
            "a valid tool-call index must not error, got: {events:?}"
        );
        assert_eq!(state.tool_calls.len(), 4, "indices 0..=3 create 4 slots");
        assert_eq!(state.tool_calls[3].name, "lookup");
        assert_eq!(state.tool_calls[3].id, "call_a");
    }

    // --- is_tools_unsupported_error (#389) --------------------------------

    #[test]
    fn tools_unsupported_matches_ollama_400() {
        // The exact provider error from #389.
        let body = r#"{"error":{"message":"registry.ollama.ai/library/smollm2:135m does not support tools","type":"invalid_request_error"}}"#;
        assert!(is_tools_unsupported_error(400, body));
    }

    #[test]
    fn tools_unsupported_is_case_insensitive() {
        assert!(is_tools_unsupported_error(
            400,
            "Model Does Not Support Tools"
        ));
    }

    #[test]
    fn tools_unsupported_ignores_other_400s() {
        // A 400 unrelated to tool support must NOT trigger the retry.
        assert!(!is_tools_unsupported_error(
            400,
            r#"{"error":{"message":"invalid api key"}}"#
        ));
    }

    #[test]
    fn tools_unsupported_ignores_non_400_status() {
        // Same wording on a non-400 status is not the case we retry.
        assert!(!is_tools_unsupported_error(
            500,
            "model does not support tools"
        ));
    }

    #[test]
    fn tools_unsupported_matches_llamacpp_jinja_400() {
        // llama.cpp started without --jinja rejects tool requests with this 400
        // (ggml-org/llama.cpp; reported via Zed, OpenCode). #389 follow-up.
        assert!(is_tools_unsupported_error(
            400,
            "tools param requires --jinja flag"
        ));
    }

    #[test]
    fn tools_unsupported_matches_llamacpp_unsupported_param_400() {
        // llama.cpp "Unsupported param: tools" (ggml-org/llama.cpp#10920).
        assert!(is_tools_unsupported_error(
            400,
            r#"{"error":{"message":"Unsupported param: tools","type":"invalid_request_error"}}"#
        ));
    }

    #[test]
    fn tools_unsupported_matches_function_calling_phrasing() {
        // Generic backends that phrase it as function calling rather than tools.
        assert!(is_tools_unsupported_error(
            400,
            "this model does not support function calling"
        ));
    }

    #[test]
    fn tools_unsupported_matches_marker_in_structured_error_message() {
        // The marker lives in `error.message` of a structured JSON body.
        let body = r#"{"error":{"message":"tools param requires --jinja flag","type":"invalid_request_error"}}"#;
        assert!(is_tools_unsupported_error(400, body));
    }

    #[test]
    fn tools_unsupported_ignores_marker_outside_error_message() {
        // A 400 for an UNRELATED reason whose body merely ECHOES a tool
        // description containing a marker phrase must NOT be classified as
        // tools-unsupported — otherwise we'd strip tools and poison the cache
        // for a model that actually supports them. We match `error.message`,
        // not the echoed request payload.
        let body = r#"{"error":{"message":"context length exceeded","type":"invalid_request_error"},"request":{"tools":[{"function":{"description":"this model does not support function calling"}}]}}"#;
        assert!(
            !is_tools_unsupported_error(400, body),
            "a marker echoed in the request payload must not trip the gate"
        );
    }

    // --- FluxRouter typed 402 / entitlement error parsing -----------------

    /// image `premium_locked`: code lives in the envelope `error.code`, the
    /// message in `error.message`. → `PremiumLocked`. (contract §2 / §3.6)
    #[test]
    fn parse_flux_402_premium_locked_image() {
        let body = r#"{"error":{"message":"image generation requires a paid plan","code":"premium_locked"}}"#;
        match parse_flux_402(body) {
            Some(ProviderError::PremiumLocked {
                capability,
                message,
            }) => {
                assert_eq!(capability, "image generation");
                assert_eq!(message, "image generation requires a paid plan");
            }
            other => panic!("expected PremiumLocked, got {other:?}"),
        }
    }

    /// web_fetch `upgrade_required`: code is the top-level `error` STRING, the
    /// message a sibling `message`. → `UpgradeRequired`. (contract §2 / §4.6)
    #[test]
    fn parse_flux_402_upgrade_required_fetch() {
        let body = r#"{"error":"upgrade_required","message":"web_fetch is a paid capability; upgrade or clear a charge"}"#;
        match parse_flux_402(body) {
            Some(ProviderError::UpgradeRequired { message }) => {
                assert_eq!(
                    message,
                    "web_fetch is a paid capability; upgrade or clear a charge"
                );
            }
            other => panic!("expected UpgradeRequired, got {other:?}"),
        }
    }

    /// money-axis chat gate `spend_ceiling_unresolved`, DOUBLY WRAPPED in the
    /// LiteLLM envelope: outer `error.message` is a *stringified* JSON object.
    /// The parser must recover the inner `error`/`reason`/`upgrade_url`. A
    /// free/no-account chat returns exactly this. (contract §2 / §5.6)
    #[test]
    fn parse_flux_402_spend_ceiling_unresolved_double_wrapped() {
        // The inner object, exactly as Flux emits it, stringified into the
        // LiteLLM envelope's `error.message`.
        let inner = r#"{"error":"spend_ceiling_unresolved","reason":"no_account_id","message":"This request requires a resolvable account spend ceiling. Add a payment method or contact billing@ferroxlabs.com.","upgrade_url":"https://fluxrouter.ai/home/billing"}"#;
        let body = serde_json::json!({
            "error": { "message": inner, "code": "402" }
        })
        .to_string();
        match parse_flux_402(&body) {
            Some(ProviderError::SpendCeilingUnresolved {
                reason,
                upgrade_url,
            }) => {
                assert_eq!(reason, "no_account_id");
                assert_eq!(
                    upgrade_url.as_deref(),
                    Some("https://fluxrouter.ai/home/billing")
                );
            }
            other => panic!("expected SpendCeilingUnresolved, got {other:?}"),
        }
    }

    /// A `spend_ceiling_unresolved` body that is NOT double-wrapped (flat
    /// top-level shape) must still parse — the parser reads inner-or-outer.
    #[test]
    fn parse_flux_402_spend_ceiling_unresolved_flat() {
        let body = r#"{"error":"spend_ceiling_unresolved","reason":"no_account_id","message":"Add a payment method.","upgrade_url":"https://fluxrouter.ai/home/billing"}"#;
        match parse_flux_402(body) {
            Some(ProviderError::SpendCeilingUnresolved {
                reason,
                upgrade_url,
            }) => {
                assert_eq!(reason, "no_account_id");
                assert_eq!(
                    upgrade_url.as_deref(),
                    Some("https://fluxrouter.ai/home/billing")
                );
            }
            other => panic!("expected SpendCeilingUnresolved, got {other:?}"),
        }
    }

    /// An unrecognised 402 (e.g. `price_exceeds_max_price`) returns `None` so
    /// the caller falls back to the generic `ProviderError::Api`.
    #[test]
    fn parse_flux_402_unrecognized_returns_none() {
        let body = r#"{"error":{"message":"final price exceeds max_price","code":"price_exceeds_max_price"}}"#;
        assert!(parse_flux_402(body).is_none());
    }

    /// A non-JSON 402 body returns `None` (falls back to `Api`).
    #[test]
    fn parse_flux_402_non_json_returns_none() {
        assert!(parse_flux_402("not json at all").is_none());
    }

    // --- #282 FluxRouter typed 409 context_overflow parsing ----------------

    /// A well-formed `context_overflow` body parses into the typed variant with
    /// every field recovered (contract V1 hard-overflow path).
    #[test]
    fn parse_flux_409_context_overflow_full() {
        let body = r#"{"error":"context_overflow","required_tokens":210000,"model_window":200000,"routed_model":"claude-sonnet-4","message":"prompt exceeds routed model window"}"#;
        match parse_flux_overflow(body) {
            Some(ProviderError::ContextOverflow {
                required_tokens,
                model_window,
                routed_model,
                message,
            }) => {
                assert_eq!(required_tokens, 210_000);
                assert_eq!(model_window, 200_000);
                assert_eq!(routed_model, "claude-sonnet-4");
                assert_eq!(message, "prompt exceeds routed model window");
            }
            other => panic!("expected ContextOverflow, got {other:?}"),
        }
    }

    /// We match on the `error` FIELD, never the status. A 409 whose `error` is
    /// something else returns `None` so it falls through to the generic `Api`.
    #[test]
    fn parse_flux_409_wrong_error_value_returns_none() {
        let body = r#"{"error":"conflict","required_tokens":1,"model_window":2,"routed_model":"x","message":"y"}"#;
        assert!(parse_flux_overflow(body).is_none());
    }

    /// A `context_overflow` body missing the numeric fields is STILL an overflow:
    /// the `error` field is authoritative, so we return `ContextOverflow` (with
    /// zeroed counts) and let the engine compact-and-retry, rather than falling
    /// through to a generic `Api` error that would kill the turn.
    #[test]
    fn parse_flux_409_missing_numeric_fields_still_overflows() {
        let body = r#"{"error":"context_overflow","message":"oops"}"#;
        match parse_flux_overflow(body) {
            Some(ProviderError::ContextOverflow {
                required_tokens,
                model_window,
                ..
            }) => {
                assert_eq!(required_tokens, 0);
                assert_eq!(model_window, 0);
            }
            other => panic!("expected ContextOverflow, got {other:?}"),
        }
    }

    /// REGRESSION (live E2E, captured from api.fluxrouter.ai 2026-06-23): the
    /// production 409 body is NOT the frozen flat shape — it is an OpenAI/LiteLLM
    /// error envelope wrapping the payload as a Python dict *repr* (single quotes)
    /// inside `error.message`. The original strict parser silently dropped this
    /// (so compact-and-retry never fired against live Flux). The parser must
    /// recover the structured fields from this exact body.
    #[test]
    fn parse_flux_409_handles_live_wrapped_python_repr_body() {
        let body = r#"{"error":{"message":"{'error': 'context_overflow', 'required_tokens': 100000256, 'model_window': 1000000, 'routed_model': 'qwen-3-coder-cerebras', 'message': 'request exceeds the window of every capable model; compact and retry'}","type":"None","param":"None","code":"409"}}"#;
        match parse_flux_overflow(body) {
            Some(ProviderError::ContextOverflow {
                required_tokens,
                model_window,
                routed_model,
                message,
            }) => {
                assert_eq!(required_tokens, 100_000_256);
                assert_eq!(model_window, 1_000_000);
                assert_eq!(routed_model, "qwen-3-coder-cerebras");
                assert!(message.contains("compact and retry"));
            }
            other => panic!("expected ContextOverflow from live body, got {other:?}"),
        }
    }

    /// The in-flight Flux 413 path raises `context_window_exceeded` via FastAPI
    /// `HTTPException(detail={…})`, which arrives either raw (`{"detail":{…}}`)
    /// or LiteLLM-wrapped. Both must classify as a context overflow so the
    /// engine compact-retries when that deploy lands. (413 carries `reason`,
    /// not `message`, and no window/routed-model — those default.)
    #[test]
    fn parse_flux_overflow_handles_413_context_window_exceeded() {
        // Raw FastAPI detail shape.
        let raw = r#"{"detail":{"error":"context_window_exceeded","reason":"Request needs ~250000 tokens but exceeds every eligible model.","required_tokens":250000}}"#;
        match parse_flux_overflow(raw) {
            Some(ProviderError::ContextOverflow {
                required_tokens,
                message,
                ..
            }) => {
                assert_eq!(required_tokens, 250_000);
                assert!(message.contains("exceeds every eligible model"));
            }
            other => panic!("expected ContextOverflow from raw 413 detail, got {other:?}"),
        }
        // LiteLLM-wrapped Python-repr of the same.
        let wrapped = r#"{"error":{"message":"{'error': 'context_window_exceeded', 'reason': 'too big', 'required_tokens': 250000}","code":"413"}}"#;
        match parse_flux_overflow(wrapped) {
            Some(ProviderError::ContextOverflow {
                required_tokens, ..
            }) => {
                assert_eq!(required_tokens, 250_000);
            }
            other => panic!("expected ContextOverflow from wrapped 413, got {other:?}"),
        }
    }

    /// A non-overflow wrapped envelope (e.g. a generic upstream error) must NOT
    /// be misread as an overflow.
    #[test]
    fn parse_flux_409_wrapped_non_overflow_returns_none() {
        let body =
            r#"{"error":{"message":"{'error': 'rate_limited', 'retry_after': 5}","code":"409"}}"#;
        assert!(parse_flux_overflow(body).is_none());
    }

    /// A malformed JSON 409 body returns `None`.
    #[test]
    fn parse_flux_409_non_json_returns_none() {
        assert!(parse_flux_overflow("not json at all").is_none());
    }

    /// `ContextOverflow` is NOT provider-retryable: the unchanged request would
    /// overflow again. The engine resolves it via compaction + an explicit
    /// single retry, so the generic retry/backoff loop must skip it.
    #[test]
    fn context_overflow_is_not_retryable() {
        let err = ProviderError::ContextOverflow {
            required_tokens: 210_000,
            model_window: 200_000,
            routed_model: "claude-sonnet-4".into(),
            message: "overflow".into(),
        };
        assert!(!err.is_retryable());
    }

    // --- #282 Flux SIGNALS-BACK x-flux-* response header parsing -----------

    /// All four `x-flux-*` headers present → a fully-populated `ProviderMeta`.
    #[test]
    fn parse_flux_response_meta_full() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-flux-routed-model"),
            HeaderValue::from_static("claude-sonnet-4"),
        );
        headers.insert(
            HeaderName::from_static("x-flux-model-window"),
            HeaderValue::from_static("200000"),
        );
        headers.insert(
            HeaderName::from_static("x-flux-context-pressure"),
            HeaderValue::from_static("0.42"),
        );
        headers.insert(
            HeaderName::from_static("x-flux-context-tokens-counted"),
            HeaderValue::from_static("84000"),
        );
        match parse_flux_response_meta(&headers) {
            Some(LlmEvent::ProviderMeta {
                routed_model,
                model_window,
                context_pressure,
                tokens_counted,
            }) => {
                assert_eq!(routed_model.as_deref(), Some("claude-sonnet-4"));
                assert_eq!(model_window, Some(200_000));
                assert_eq!(context_pressure, Some(0.42));
                assert_eq!(tokens_counted, Some(84_000));
            }
            other => panic!("expected ProviderMeta, got {other:?}"),
        }
    }

    /// A non-Flux response (NONE of the headers) yields `None`, so the engine
    /// emits nothing on non-Flux routes.
    #[test]
    fn parse_flux_response_meta_absent_returns_none() {
        let headers = HeaderMap::new();
        assert!(parse_flux_response_meta(&headers).is_none());
    }

    /// A partial signal (only the window) still emits a `ProviderMeta`; the
    /// missing fields are `None`, never a parse error.
    #[test]
    fn parse_flux_response_meta_partial_window_only() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-flux-model-window"),
            HeaderValue::from_static("128000"),
        );
        match parse_flux_response_meta(&headers) {
            Some(LlmEvent::ProviderMeta {
                routed_model,
                model_window,
                context_pressure,
                tokens_counted,
            }) => {
                assert_eq!(model_window, Some(128_000));
                assert!(routed_model.is_none());
                assert!(context_pressure.is_none());
                assert!(tokens_counted.is_none());
            }
            other => panic!("expected ProviderMeta, got {other:?}"),
        }
    }

    /// The typed variants render DISTINCT Display messages: the two feature
    /// locks vs the account-needs-payment state must be separable by the CLI.
    #[test]
    fn flux_402_variants_render_distinct_messages() {
        let locked = ProviderError::PremiumLocked {
            capability: "image generation".into(),
            message: "requires a paid plan".into(),
        }
        .to_string();
        let upgrade = ProviderError::UpgradeRequired {
            message: "web_fetch is paid".into(),
        }
        .to_string();
        let spend = ProviderError::SpendCeilingUnresolved {
            reason: "no_account_id".into(),
            upgrade_url: Some("https://fluxrouter.ai/home/billing".into()),
        }
        .to_string();

        // Feature locks read as a plan/upgrade requirement.
        assert!(locked.contains("paid Flux plan"), "got: {locked}");
        assert!(upgrade.contains("requires an upgrade"), "got: {upgrade}");
        // The account state reads as "add a payment method" and surfaces the URL.
        assert!(spend.contains("needs a payment method"), "got: {spend}");
        assert!(
            spend.contains("https://fluxrouter.ai/home/billing"),
            "got: {spend}"
        );
        // The three messages are mutually distinct.
        assert_ne!(locked, upgrade);
        assert_ne!(locked, spend);
        assert_ne!(upgrade, spend);
    }

    /// All three Flux 402 entitlement errors are terminal (not retryable):
    /// retrying the same request on the same key 402s again.
    #[test]
    fn flux_402_errors_are_not_retryable() {
        assert!(
            !ProviderError::PremiumLocked {
                capability: "image generation".into(),
                message: String::new(),
            }
            .is_retryable()
        );
        assert!(
            !ProviderError::UpgradeRequired {
                message: String::new(),
            }
            .is_retryable()
        );
        assert!(
            !ProviderError::SpendCeilingUnresolved {
                reason: "no_account_id".into(),
                upgrade_url: None,
            }
            .is_retryable()
        );
    }

    // --- D008: live /model library — /v1/models parse + url derivation ----

    #[test]
    fn parse_openai_models_extracts_ids_from_data_array() {
        // The documented OpenAI shape: {"data":[{"id":...,"object":"model"}]}.
        let body = r#"{"object":"list","data":[
            {"id":"gpt-4o","object":"model","owned_by":"openai"},
            {"id":"gpt-4o-mini","object":"model","owned_by":"openai"}
        ]}"#;
        let models = parse_openai_models(body).expect("valid body parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        // OpenAI returns no display name — display mirrors id.
        assert_eq!(models[0].display, "gpt-4o");
        assert_eq!(models[1].id, "gpt-4o-mini");
    }

    #[test]
    fn parse_openai_models_skips_entries_without_a_valid_id() {
        // Defensive: an entry missing `id`, or with a non-string/empty id,
        // must be skipped rather than producing a bogus model.
        let body = r#"{"data":[
            {"id":"gpt-4o"},
            {"object":"model"},
            {"id":""},
            {"id":123}
        ]}"#;
        let models = parse_openai_models(body).expect("parses");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-4o");
    }

    #[test]
    fn parse_openai_models_errors_when_data_missing() {
        // No `data` array → Err, so the caller falls back to the alias list.
        let body = r#"{"error":{"message":"unauthorized"}}"#;
        assert!(parse_openai_models(body).is_err());
        // Non-JSON also errors.
        assert!(parse_openai_models("not json").is_err());
    }

    #[test]
    fn models_url_default_openai_base() {
        // Native OpenAI: base has no /v1, default api_path is
        // /v1/chat/completions → /v1/models.
        let p = OpenAIProvider::new(
            "key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
        );
        assert_eq!(p.models_url(), "https://api.openai.com/v1/models");
    }

    #[test]
    fn models_url_base_with_v1_and_overridden_api_path() {
        // Catalog/Together style: base already ends in /v1 and api_path is
        // /chat/completions → strip suffix, append /models → /v1/models.
        let compat = ProviderCompat {
            api_path: Some("/chat/completions".into()),
            ..Default::default()
        };
        let p = OpenAIProvider::new(
            "key",
            "https://api.together.xyz/v1",
            compat,
            DebugConfig::default(),
        );
        assert_eq!(p.models_url(), "https://api.together.xyz/v1/models");
    }

    #[test]
    fn models_url_falls_back_to_v1_models_for_unusual_path() {
        // An api_path with no /chat/completions suffix falls back to the
        // canonical /v1/models under the base.
        let compat = ProviderCompat {
            api_path: Some("/responses".into()),
            ..Default::default()
        };
        let p = OpenAIProvider::new(
            "key",
            "https://example.test",
            compat,
            DebugConfig::default(),
        );
        assert_eq!(p.models_url(), "https://example.test/v1/models");
    }

    // --- Responses API routing (gpt-5) ------------------------------------

    #[test]
    fn responses_url_default_openai_base() {
        // Native OpenAI: base has no /v1, default api_path is
        // /v1/chat/completions → /v1/responses.
        let p = OpenAIProvider::new(
            "key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
        );
        assert_eq!(
            p.responses_url_for(&p.base_url),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn responses_url_base_with_v1_and_overridden_api_path() {
        // Catalog style: base ends in /v1, api_path is /chat/completions →
        // strip suffix, append /responses → /v1/responses.
        let compat = ProviderCompat {
            api_path: Some("/chat/completions".into()),
            ..Default::default()
        };
        let p = OpenAIProvider::new(
            "key",
            "https://api.example.com/v1",
            compat,
            DebugConfig::default(),
        );
        assert_eq!(
            p.responses_url_for(&p.base_url),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn uses_responses_api_routes_gpt5_only_by_default() {
        let p = OpenAIProvider::new(
            "key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
        );
        let mut req = LlmRequest {
            model: "gpt-5".into(),
            ..Default::default()
        };
        assert!(p.uses_responses_api(&req), "gpt-5 must route to Responses");
        req.model = "gpt-4o".into();
        assert!(
            !p.uses_responses_api(&req),
            "gpt-4o must stay on Chat Completions"
        );
        req.model = "o1-mini".into();
        assert!(
            !p.uses_responses_api(&req),
            "o-series stays on Chat Completions"
        );
    }

    #[test]
    fn uses_responses_api_honors_compat_override() {
        // Override forces gpt-5 back onto Chat Completions (gateway proxy).
        let compat = ProviderCompat {
            uses_responses_api: Some(false),
            ..ProviderCompat::openai_defaults()
        };
        let p = OpenAIProvider::new(
            "key",
            "https://gateway.example.com/v1",
            compat,
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-5".into(),
            ..Default::default()
        };
        assert!(
            !p.uses_responses_api(&req),
            "compat override Some(false) forces Chat Completions"
        );
    }

    #[test]
    fn build_request_body_for_gpt5_is_only_built_on_chat_path() {
        // Sanity: the chat-path body builder still uses max_completion_tokens
        // for gpt-5 (used only when an override forces gpt-5 onto chat).
        let p = OpenAIProvider::new(
            "key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-5".into(),
            max_tokens: 1024,
            ..Default::default()
        };
        let body = p.build_request_body(&req);
        assert_eq!(body["max_completion_tokens"], 1024);
        assert!(body.get("max_tokens").is_none());
    }

    // --- map_openai_finish_reason (Task F) --------------------------------

    #[test]
    fn test_map_openai_stop_to_stop() {
        assert_eq!(map_openai_finish_reason("stop"), FinishReason::Stop);
    }

    #[test]
    fn test_map_openai_tool_calls_to_stop() {
        assert_eq!(map_openai_finish_reason("tool_calls"), FinishReason::Stop);
    }

    #[test]
    fn test_map_openai_length_to_length() {
        // Gemini Pro reasoning models surface "length" when thinking-token
        // budget is exhausted — the protocol must propagate that distinctly.
        assert_eq!(map_openai_finish_reason("length"), FinishReason::Length);
    }

    #[test]
    fn test_map_openai_content_filter_to_error() {
        assert_eq!(
            map_openai_finish_reason("content_filter"),
            FinishReason::Error
        );
    }

    #[test]
    fn test_map_openai_unknown_to_error() {
        assert_eq!(
            map_openai_finish_reason("garbage_future_value"),
            FinishReason::Error
        );
    }

    #[test]
    fn test_openai_length_finish_reason_emits_finishreason_length() {
        // End-to-end: when OpenAI emits finish_reason=length the pending
        // Done event should carry FinishReason::Length (the Gemini Pro
        // reasoning-token bug signature, surfaced via openai-compat).
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert!(events.is_empty(), "Done is deferred until [DONE]");
        let done = state.flush_done().expect("pending_done should be set");
        match done {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                ..
            } => {
                assert_eq!(stop_reason, StopReason::MaxTokens);
                assert_eq!(finish_reason, FinishReason::Length);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_openai_content_filter_finish_reason_emits_finishreason_error() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert!(events.is_empty(), "Done is deferred until [DONE]");
        let done = state.flush_done().expect("pending_done should be set");
        match done {
            LlmEvent::Done { finish_reason, .. } => {
                assert_eq!(finish_reason, FinishReason::Error);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    fn openai_compat() -> ProviderCompat {
        ProviderCompat::openai_defaults()
    }

    // --- message_compat (#417: per-request reasoning_content replay) ---

    #[test]
    fn message_compat_forces_replay_for_strict_reasoner_via_router() {
        // Flux's static compat has replay OFF, but routing to DeepSeek must
        // turn it ON for this request so reasoning_content is replayed (#417).
        let flux = ProviderCompat::flux_router_defaults();
        assert!(
            !flux.replays_thinking_in_history(),
            "precondition: router compat has replay off"
        );
        let resolved = OpenAIProvider::message_compat(&flux, "deepseek-v4-pro");
        assert!(
            resolved.replays_thinking_in_history(),
            "DeepSeek via Flux must replay reasoning_content"
        );
    }

    #[test]
    fn message_compat_does_not_replay_for_non_strict_via_router() {
        // claude-via-Flux must NOT replay — Anthropic 400s on an unsigned
        // thinking block. Ordinary OpenAI models stay off too.
        let flux = ProviderCompat::flux_router_defaults();
        assert!(
            !OpenAIProvider::message_compat(&flux, "claude-opus-4-7").replays_thinking_in_history()
        );
        assert!(!OpenAIProvider::message_compat(&flux, "gpt-4o").replays_thinking_in_history());
    }

    #[test]
    fn message_compat_is_noop_for_direct_deepseek() {
        // Direct DeepSeek already sets the flag; resolution leaves it on.
        let ds = ProviderCompat::deepseek_defaults();
        assert!(ds.replays_thinking_in_history());
        assert!(
            OpenAIProvider::message_compat(&ds, "deepseek-v4-pro").replays_thinking_in_history()
        );
    }

    // --- max_tokens_field ---

    #[test]
    fn test_max_tokens_field_default() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
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
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    /// #112: when the engine flags `omit_max_tokens` AND this provider's own
    /// compat is omit-safe, the chat body carries NEITHER `max_tokens` NOR
    /// `max_completion_tokens` — the served model's natural ceiling applies.
    #[test]
    fn test_omit_max_tokens_drops_the_wire_field() {
        // Omit-safe provider compat (flux-router preset).
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            ProviderCompat::flux_router_defaults(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "flux-auto".into(),
            max_tokens: 8_192, // sized internal budget stays positive
            omit_max_tokens: true,
            ..Default::default()
        };
        let body = provider.build_request_body(&req);
        assert!(
            body.get("max_tokens").is_none(),
            "omit_max_tokens must drop max_tokens from the wire body"
        );
        assert!(body.get("max_completion_tokens").is_none());

        // And a reasoning-family model (max_completion_tokens path) omits too.
        let req_reasoning = LlmRequest {
            model: "o3-pro-unlisted".into(),
            max_tokens: 32_768,
            omit_max_tokens: true,
            ..Default::default()
        };
        let body = provider.build_request_body(&req_reasoning);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    /// #112 belt-and-braces: the request flag alone is NOT enough — a provider
    /// whose OWN compat is not omit-safe (plain openai / generic openai-compat)
    /// keeps sending the sized value even if a cross-compat request arrives
    /// with `omit_max_tokens = true`.
    #[test]
    fn test_omit_max_tokens_ignored_when_provider_compat_not_omit_safe() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(), // openai_defaults: omit_max_tokens_when_unsized off
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "some-self-hosted-model".into(),
            max_tokens: 8_192,
            omit_max_tokens: true,
            ..Default::default()
        };
        let body = provider.build_request_body(&req);
        assert_eq!(
            body["max_tokens"], 8_192,
            "a non-omit-safe provider must keep sending the sized value"
        );
    }

    // --- Output-side opt (Part A): stop_sequences -> body["stop"] ----------

    fn stop_provider() -> OpenAIProvider {
        OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        )
    }

    fn stop_req() -> LlmRequest {
        LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
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
    fn build_request_body_omits_stop_when_empty() {
        let body = stop_provider().build_request_body(&stop_req());
        assert!(
            body.get("stop").is_none(),
            "empty stop_sequences must emit no `stop` field (back-compat)"
        );
    }

    // --- #86: stream_options gate for generic OpenAI-compatible endpoints ---

    #[test]
    fn build_request_body_emits_stream_options_by_default() {
        // Default keeps stream_options:{include_usage:true} for token accounting.
        let body = stop_provider().build_request_body(&stop_req());
        assert_eq!(body["stream_options"]["include_usage"], json!(true));
    }

    #[test]
    fn build_request_body_omits_stream_options_when_disabled() {
        // A generic self-hosted endpoint that 400s on the unknown
        // `stream_options` field can suppress it via compat.
        let mut compat = openai_compat();
        compat.include_usage_in_stream = Some(false);
        let provider =
            OpenAIProvider::new("key", "http://localhost", compat, DebugConfig::default());
        let body = provider.build_request_body(&stop_req());
        assert!(
            body.get("stream_options").is_none(),
            "stream_options must be omitted when include_usage_in_stream = false"
        );
    }

    #[test]
    fn build_request_body_emits_stop_when_present() {
        let mut req = stop_req();
        req.stop_sequences = vec!["\n\nLet me know if".into(), "\n\nFeel free to".into()];
        let body = stop_provider().build_request_body(&req);
        assert_eq!(
            body["stop"],
            json!(["\n\nLet me know if", "\n\nFeel free to"]),
            "OpenAI must emit stops under the `stop` key as an array"
        );
    }

    // --- SPEC-V2 CORE-1: Flux sticky-session prompt_cache_key -------------

    /// On a Flux tier alias with a conversation id, the body carries
    /// `prompt_cache_key = conversation_id` so the router can pin the
    /// conversation to one backend (cache affinity) and OpenAI-compatible
    /// upstreams get their cache-routing hint.
    #[test]
    fn build_request_body_stamps_prompt_cache_key_on_tier_alias() {
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.conversation_id = Some("conv-abc".into());
        let body = stop_provider().build_request_body(&req);
        assert_eq!(body["prompt_cache_key"], json!("conv-abc"));
    }

    /// A concrete model id opts OUT: the body must stay byte-identical to
    /// today (generic OpenAI-compatible endpoints can 400 on unknown fields).
    #[test]
    fn build_request_body_no_prompt_cache_key_for_concrete_model() {
        let mut req = stop_req();
        req.model = "gpt-4o".into();
        req.conversation_id = Some("conv-abc".into());
        let body = stop_provider().build_request_body(&req);
        assert!(body.get("prompt_cache_key").is_none());
    }

    /// Absent (or empty) conversation_id skips the field entirely — never
    /// emit an empty cache key.
    #[test]
    fn build_request_body_no_prompt_cache_key_without_conversation_id() {
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.conversation_id = None;
        let body = stop_provider().build_request_body(&req);
        assert!(body.get("prompt_cache_key").is_none());

        req.conversation_id = Some(String::new());
        let body = stop_provider().build_request_body(&req);
        assert!(body.get("prompt_cache_key").is_none());

        // Whitespace-only ids are degenerate cache keys too (GLM-5.2 audit).
        req.conversation_id = Some("   ".into());
        let body = stop_provider().build_request_body(&req);
        assert!(body.get("prompt_cache_key").is_none());
    }

    // --- Crucible #3: per-tier temperature emission -----------------------

    #[test]
    fn build_request_body_emits_temperature_for_accepting_model() {
        // A proposer-tier temperature on an accepting model (gpt-4o) is emitted.
        let mut req = stop_req();
        req.temperature = Some(0.6);
        let body = stop_provider().build_request_body(&req);
        assert!(
            (body["temperature"].as_f64().unwrap() - 0.6).abs() < 1e-6,
            "accepting model + Some(0.6) must emit temperature 0.6 (proposer tier)"
        );

        // The aggregator-tier temperature is emitted identically.
        let mut agg = stop_req();
        agg.temperature = Some(0.4);
        let agg_body = stop_provider().build_request_body(&agg);
        assert!(
            (agg_body["temperature"].as_f64().unwrap() - 0.4).abs() < 1e-6,
            "accepting model + Some(0.4) must emit temperature 0.4 (aggregator tier)"
        );
    }

    #[test]
    fn build_request_body_omits_temperature_when_unset() {
        // `None` (the back-compat default) emits no temperature field.
        let body = stop_provider().build_request_body(&stop_req());
        assert!(
            body.get("temperature").is_none(),
            "temperature: None must emit no `temperature` field (back-compat)"
        );
    }

    #[test]
    fn build_request_body_omits_temperature_for_o_series_model() {
        // o1/o3 reasoning families fix temperature at 1.0 and reject an explicit
        // value, so `accepts_temperature` drops it even when a council temp is set.
        let mut req = stop_req();
        req.model = "o1-mini".into();
        req.temperature = Some(0.6);
        let body = stop_provider().build_request_body(&req);
        assert!(
            body.get("temperature").is_none(),
            "o1-class model must omit temperature even when a council temp is set"
        );
    }

    #[test]
    fn build_request_body_omits_temperature_when_compat_opts_out() {
        // A provider with supports_temperature = false drops the field entirely.
        let mut compat = openai_compat();
        compat.supports_temperature = Some(false);
        let provider =
            OpenAIProvider::new("key", "http://localhost", compat, DebugConfig::default());
        let mut req = stop_req();
        req.temperature = Some(0.6);
        let body = provider.build_request_body(&req);
        assert!(
            body.get("temperature").is_none(),
            "supports_temperature = false must omit the temperature field"
        );
    }

    // --- FluxRouter web_search grounding (contract §5) --------------------

    /// The tier-alias guard accepts exactly the four documented aliases
    /// (case-insensitive) and rejects concrete model ids (contract §5.2/§5.8).
    #[test]
    fn is_flux_tier_alias_accepts_only_the_four_aliases() {
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            assert!(is_flux_tier_alias(alias), "{alias} must be a tier alias");
            assert!(
                is_flux_tier_alias(&alias.to_uppercase()),
                "{alias} must match case-insensitively"
            );
        }
        for concrete in [
            "gpt-5",
            "kimi-k2-6",
            "claude-sonnet-4",
            "flux-pinned-sonar",
            "",
        ] {
            assert!(
                !is_flux_tier_alias(concrete),
                "{concrete} must NOT be treated as a tier alias"
            );
        }
    }

    // --- #282 Core→Flux context-routing request headers -------------------

    /// On a Flux **tier alias** the x-wl-* context-routing headers are emitted:
    /// context tokens + expected output (= max_tokens) + the literal managed
    /// opt-in + the conversation id. (#282 contract V1, emit side.)
    #[test]
    fn flux_context_headers_emitted_for_tier_alias() {
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.max_tokens = 4096;
        req.conversation_id = Some("conv-abc".into());
        req.client_context_tokens = Some(123_456);

        let mut headers = HeaderMap::new();
        OpenAIProvider::apply_flux_context_headers(&mut headers, &req);

        assert_eq!(
            headers.get("x-wl-context-tokens").unwrap(),
            "123456",
            "context tokens header carries the assembled-prompt estimate"
        );
        assert_eq!(
            headers.get("x-wl-expected-output").unwrap(),
            "4096",
            "expected-output header carries request.max_tokens"
        );
        assert_eq!(
            headers.get("x-wl-context-managed").unwrap(),
            "true",
            "managed opt-in is the literal `true`"
        );
        assert_eq!(headers.get("x-wl-conversation-id").unwrap(), "conv-abc");
    }

    /// A CONCRETE model id (the customer pinned the upstream) opts OUT: NONE of
    /// the x-wl-* headers are emitted, so non-Flux / pinned requests are
    /// byte-for-byte unchanged. (#282 gating proof.)
    #[test]
    fn flux_context_headers_absent_for_concrete_model() {
        let mut req = stop_req();
        req.model = "gpt-4o".into();
        req.conversation_id = Some("conv-abc".into());
        req.client_context_tokens = Some(123_456);

        let mut headers = HeaderMap::new();
        OpenAIProvider::apply_flux_context_headers(&mut headers, &req);

        assert!(headers.get("x-wl-context-tokens").is_none());
        assert!(headers.get("x-wl-expected-output").is_none());
        assert!(headers.get("x-wl-context-managed").is_none());
        assert!(headers.get("x-wl-conversation-id").is_none());
        assert!(
            headers.is_empty(),
            "a concrete-model request must carry no x-wl-* headers at all"
        );
    }

    /// When the optional values are absent the header is SKIPPED (not emitted
    /// empty), but the always-present managed opt-in + expected-output remain.
    #[test]
    fn flux_context_headers_skip_absent_optionals() {
        let mut req = stop_req();
        req.model = "flux-reasoning".into();
        req.conversation_id = None;
        req.client_context_tokens = None;

        let mut headers = HeaderMap::new();
        OpenAIProvider::apply_flux_context_headers(&mut headers, &req);

        assert!(headers.get("x-wl-context-tokens").is_none());
        assert!(headers.get("x-wl-conversation-id").is_none());
        assert_eq!(headers.get("x-wl-context-managed").unwrap(), "true");
        assert!(headers.get("x-wl-expected-output").is_some());
    }

    /// #112: omitting the wire max-tokens field must NOT starve the Flux
    /// headers — `x-wl-expected-output` still carries the sized internal
    /// budget even when the body field is omitted.
    #[test]
    fn flux_expected_output_header_survives_omit_max_tokens() {
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.max_tokens = 8_192;
        req.omit_max_tokens = true;

        let mut headers = HeaderMap::new();
        OpenAIProvider::apply_flux_context_headers(&mut headers, &req);
        assert_eq!(
            headers.get("x-wl-expected-output").unwrap(),
            "8192",
            "the sized internal budget must still ride the header"
        );
    }

    /// web_search on a tier-alias model injects the `{"type":"web_search"}`
    /// tool and DROPS the function tools (function tools suppress grounding,
    /// contract §5.8).
    #[test]
    fn web_search_injects_tool_on_tier_alias_and_drops_function_tools() {
        let provider = stop_provider();
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.web_search = true;
        req.tools = vec![ToolDef {
            name: "Read".into(),
            description: "read a file".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            server: None,
        }];
        let body = provider.build_request_body(&req);
        let tools = body["tools"].as_array().expect("tools array present");
        assert_eq!(tools.len(), 1, "only the web_search tool survives");
        assert_eq!(tools[0]["type"], "web_search");
        // The function tool must NOT be present (no `function` key anywhere).
        assert!(
            tools.iter().all(|t| t.get("function").is_none()),
            "function tools must be dropped when grounding"
        );
    }

    /// web_search on a CONCRETE model id does NOT inject the tool — grounding
    /// would not fire there, so the normal function-tool path is preserved.
    #[test]
    fn web_search_absent_for_concrete_model() {
        let provider = stop_provider();
        let mut req = stop_req();
        req.model = "gpt-4o".into();
        req.web_search = true;
        req.tools = vec![ToolDef {
            name: "Read".into(),
            description: "read a file".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            server: None,
        }];
        let body = provider.build_request_body(&req);
        let tools = body["tools"].as_array().expect("tools array present");
        // The web_search tool must be ABSENT; the function tool is kept.
        assert!(
            tools.iter().all(|t| t["type"] != "web_search"),
            "concrete model must not get a web_search tool"
        );
        assert!(
            tools.iter().any(|t| t.get("function").is_some()),
            "function tools are preserved on the concrete-model path"
        );
    }

    /// web_search unset → no web_search tool even on a tier alias.
    #[test]
    fn web_search_unset_injects_nothing() {
        let provider = stop_provider();
        let mut req = stop_req();
        req.model = "flux-auto".into();
        req.web_search = false;
        let body = provider.build_request_body(&req);
        assert!(
            body.get("tools").is_none(),
            "no tools at all when web_search is off and no function tools given"
        );
    }

    /// Contract §5.4: a synthetic streamed frame carrying TOP-LEVEL `citations`
    /// / `search_results` (alongside `choices`) accumulates in StreamState and
    /// emits `Citations` + `SearchResults` events when flushed at end-of-stream.
    /// ⚠️ Frame placement here is the UNVERIFIED documented shape — see the
    /// note on `StreamState::merge_flux_grounding`.
    #[test]
    fn parse_sse_chunk_accumulates_top_level_citations() {
        let frame = json!({
            "id": "x",
            "object": "chat.completion.chunk",
            "model": "perplexity/sonar",
            "choices": [{ "index": 0, "delta": { "content": "JWST [1]" } }],
            "citations": ["https://science.nasa.gov/jwst", "https://esawebb.org/news/"],
            "search_results": [
                { "title": "JWST", "url": "https://science.nasa.gov/jwst",
                  "date": "2026-06-15", "snippet": "…", "source": "web" }
            ]
        })
        .to_string();
        let mut state = StreamState::new();
        let _ = parse_sse_chunk(&frame, &mut state);
        assert_eq!(state.citations.len(), 2);
        assert_eq!(state.citations[0], "https://science.nasa.gov/jwst");
        assert_eq!(state.search_results.len(), 1);
        assert_eq!(state.search_results[0].title, "JWST");
        assert_eq!(state.search_results[0].date.as_deref(), Some("2026-06-15"));
    }

    /// Citations / search_results repeated across frames are deduped: a second
    /// frame re-sending the same URLs must not double them.
    #[test]
    fn parse_sse_chunk_dedupes_citations_across_frames() {
        let frame = json!({
            "choices": [],
            "citations": ["https://a.example", "https://b.example"],
            "search_results": [{ "title": "A", "url": "https://a.example", "snippet": "", "source": "web" }]
        })
        .to_string();
        let mut state = StreamState::new();
        let _ = parse_sse_chunk(&frame, &mut state);
        let _ = parse_sse_chunk(&frame, &mut state);
        assert_eq!(state.citations.len(), 2, "duplicate URLs collapse");
        assert_eq!(
            state.search_results.len(),
            1,
            "duplicate cards collapse on url"
        );
    }

    /// A normal (ungrounded) frame leaves the grounding accumulators empty, so
    /// non-Flux turns never emit Citations / SearchResults.
    #[test]
    fn parse_sse_chunk_ungrounded_frame_has_no_citations() {
        let frame = json!({
            "choices": [{ "index": 0, "delta": { "content": "hello" } }]
        })
        .to_string();
        let mut state = StreamState::new();
        let _ = parse_sse_chunk(&frame, &mut state);
        assert!(state.citations.is_empty());
        assert!(state.search_results.is_empty());
    }

    // --- Groq Compound: tools omitted for agentic models -------------------

    fn one_tool() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
            server: None,
        }]
    }

    #[test]
    fn build_request_body_emits_tools_for_normal_model() {
        let mut req = stop_req();
        req.tools = one_tool();
        let body = stop_provider().build_request_body(&req);
        assert!(
            body.get("tools")
                .and_then(|t| t.as_array())
                .is_some_and(|a| a.len() == 1),
            "a normal model must carry the caller's tools"
        );
    }

    #[test]
    fn build_request_body_omits_tools_for_groq_compound() {
        // Groq Compound 400s on a `tools` array — the engine must omit it so the
        // turn succeeds instead of breaking.
        let mut req = stop_req();
        req.model = "compound-beta".into();
        req.tools = one_tool();
        let body = stop_provider().build_request_body(&req);
        assert!(
            body.get("tools").is_none(),
            "Groq Compound must receive NO `tools` field (it rejects tool calling)"
        );
    }

    #[test]
    fn build_request_body_omits_tools_when_capability_cache_says_unsupported() {
        // Capability-first gate (#389/#97 follow-up): a model that PASSES the
        // name-gate still gets its `tools` array dropped once the per-provider
        // capability cache records it as tools-unsupported (set by the Ollama
        // `/api/show` probe or a reactive 400). Proves the cache — not just the
        // static name-gate — gates `body["tools"]`.
        let provider = stop_provider();
        let mut req = stop_req();
        req.tools = one_tool();

        // With an empty cache the (normal) model carries its tools optimistically.
        assert!(
            provider
                .build_request_body(&req)
                .get("tools")
                .and_then(|t| t.as_array())
                .is_some_and(|a| a.len() == 1),
            "an unknown model must carry tools optimistically"
        );

        // Record the model as tools-unsupported; the array must now be dropped.
        provider.tool_support.set(&req.model, false);
        assert!(
            provider.build_request_body(&req).get("tools").is_none(),
            "a model cached as tools-unsupported must receive NO `tools` field"
        );
    }

    #[test]
    fn test_max_tokens_field_custom() {
        let compat = ProviderCompat {
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        };
        let provider =
            OpenAIProvider::new("key", "http://localhost", compat, DebugConfig::default());
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 2048,
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
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    // --- T0: per-request model-family request-body shape ------------------
    // Closes the live v0.6.5 desktop bug: one OpenAIProvider instance
    // serves both gpt-4o (`max_tokens`) and gpt-5 (`max_completion_tokens`)
    // in the same session. The shape MUST be chosen per request, not per
    // provider construction.

    /// gpt-5 family requires `max_completion_tokens` — the engine must
    /// flip the field even when the provider was constructed with the
    /// classic `openai_defaults()` compat.
    #[test]
    fn test_build_request_body_gpt5_emits_max_completion_tokens() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-5".into(),
            system: String::new(),
            messages: vec![],
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
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_completion_tokens"], 1024);
        assert!(
            body.get("max_tokens").is_none(),
            "gpt-5 must NOT carry max_tokens — OpenAI 400s on it"
        );
    }

    /// gpt-4o stays on classic `max_tokens` even though the same provider
    /// instance just served a gpt-5 request above.
    #[test]
    fn test_build_request_body_gpt4o_emits_max_tokens() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
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
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    /// `reasoning_effort` is silently dropped when the target model
    /// doesn't accept it. gpt-4o 400s on the field, so we must not emit
    /// it even if the caller passed one.
    #[test]
    fn test_build_request_body_gpt4o_drops_reasoning_effort() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: Some("medium".into()),
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        };
        let body = provider.build_request_body(&req);
        assert!(
            body.get("reasoning_effort").is_none(),
            "gpt-4o must NOT carry reasoning_effort — OpenAI 400s on it"
        );
    }

    /// o1-mini accepts `reasoning_effort` — it must be forwarded.
    #[test]
    fn test_build_request_body_o1_mini_keeps_reasoning_effort() {
        let provider = OpenAIProvider::new(
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "o1-mini".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: Some("medium".into()),
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["reasoning_effort"], "medium");
        // And it must also use `max_completion_tokens` on the o-series.
        assert_eq!(body["max_completion_tokens"], 1024);
        assert!(body.get("max_tokens").is_none());
    }

    // --- merge_assistant_messages ---

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    #[test]
    fn test_build_messages_user_image_becomes_multipart() {
        // A user turn with an inline image lowers to the OpenAI Chat multi-part
        // array shape: a text part + an image_url data-URI part.
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "describe".into(),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "QUJD".into(),
                },
            ],
        )];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let user = result.iter().find(|m| m["role"] == "user").unwrap();
        let parts = user["content"].as_array().expect("content must be array");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "describe");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,QUJD");
    }

    #[test]
    fn test_build_messages_user_without_image_stays_string() {
        // Backwards-compat: no image → plain string content (unchanged wire shape).
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let user = result.iter().find(|m| m["role"] == "user").unwrap();
        assert_eq!(user["content"], "hi");
    }

    #[test]
    fn test_build_messages_non_vision_omits_image_and_emits_placeholder() {
        // #648: a text-only (non-vision) compat model MUST NOT emit an
        // `image_url` multipart part — text-only endpoints 400 on it. Instead
        // the image is dropped and the shared placeholder text is appended, so
        // the content stays a plain string (soft degradation, no hard reject).
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "describe".into(),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "QUJD".into(),
                },
            ],
        )];
        // `no_compat()` is `ProviderCompat::default()` → `supports_vision()` false.
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let user = result.iter().find(|m| m["role"] == "user").unwrap();

        // Content is the plain-string form, NOT a multipart array.
        assert!(
            user["content"].is_string(),
            "non-vision content must stay a plain string, got: {}",
            user["content"]
        );
        let content = user["content"].as_str().unwrap();
        assert_eq!(
            content,
            "describe\n[image omitted: model not vision-capable]"
        );

        // Belt-and-suspenders: the serialized message must carry no image_url.
        assert!(
            !user.to_string().contains("image_url"),
            "non-vision message must not serialize an image_url part"
        );
    }

    // --- replays_thinking_in_history (finding #174) ---

    /// A history with a prior-turn assistant thinking block must NOT re-emit
    /// that thinking as `reasoning_content` on the default OpenAI chat path —
    /// historical thinking the model does not need is billed as fresh input
    /// every turn, so we drop it at the wire (matching Anthropic). DeepSeek/Kimi
    /// are the only providers that opt back in via the compat flag.
    ///
    /// Without the fix this test FAILS: the old code set `has_any_thinking` from
    /// any assistant Thinking block and unconditionally wrote `reasoning_content`
    /// onto EVERY assistant message, so the assertion that no assistant message
    /// carries `reasoning_content` would not hold for the default OpenAI preset.
    #[test]
    fn historical_thinking_dropped_on_default_openai_path() {
        let messages = vec![
            // Prior assistant turn that produced thinking + text.
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "secret prior-turn reasoning".into(),
                    },
                    ContentBlock::Text {
                        text: "answer one".into(),
                    },
                ],
            ),
            // Follow-up user turn (forces the prior assistant turn to be history).
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "and again?".into(),
                }],
            ),
        ];

        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());

        // No assistant message may carry reasoning_content on the default path,
        // and the historical thinking text must not appear anywhere in the body.
        for m in &result {
            if m["role"] == "assistant" {
                assert!(
                    m.get("reasoning_content").is_none(),
                    "default OpenAI path must not replay historical reasoning_content: {m:?}"
                );
            }
        }
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(
            !serialized.contains("secret prior-turn reasoning"),
            "historical thinking must not be re-sent (re-billed) as input"
        );
        // Sanity: the actual assistant text is still carried.
        assert!(serialized.contains("answer one"));
    }

    /// DeepSeek/Kimi require the replay (their API 400s without it), so the
    /// compat flag re-enables emission of `reasoning_content` on every assistant
    /// message once any turn has thinking. This guards the strict-reasoner path
    /// from regressing when we drop historical thinking everywhere else.
    #[test]
    fn historical_thinking_replayed_for_deepseek() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "prior reasoning".into(),
                    },
                    ContentBlock::Text {
                        text: "answer one".into(),
                    },
                ],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "again?".into(),
                }],
            ),
        ];

        let compat = ProviderCompat::deepseek_defaults();
        let result = OpenAIProvider::build_messages(&messages, "", &compat);

        let assistant = result
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message present");
        assert_eq!(
            assistant["reasoning_content"], "prior reasoning",
            "DeepSeek must replay historical reasoning_content (API requires it)"
        );
    }

    // --- clean_orphan_tool_calls ---

    #[test]
    fn test_clean_orphan_tool_calls_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            // tc2 has no result -> orphan
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "tc1");
    }

    #[test]
    fn test_clean_orphan_tool_calls_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    // --- clean_orphan_tool_results (FerroxLabs/wayland#85) ---

    #[test]
    fn test_clean_orphan_tool_results_enabled() {
        // A `tool` result whose tool_call_id has no parent assistant
        // `tool_calls` entry (e.g. the parent message was trimmed from the
        // history) must be stripped — OpenAI-format APIs 400 otherwise.
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            // Orphan: its parent assistant tool_calls entry is absent.
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc_ghost".into(),
                    content: "orphaned".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "tc1");

        // Invariant the provider 400 was caused by: every surviving tool
        // result resolves to an assistant tool_calls id.
        let called: std::collections::HashSet<String> = result
            .iter()
            .filter(|m| m["role"] == "assistant")
            .filter_map(|m| m["tool_calls"].as_array())
            .flatten()
            .filter_map(|tc| tc["id"].as_str().map(String::from))
            .collect();
        for tm in &tool_msgs {
            assert!(called.contains(tm["tool_call_id"].as_str().unwrap()));
        }
    }

    #[test]
    fn test_clean_orphan_tool_results_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc_ghost".into(),
                    content: "orphaned".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 2);
    }

    // --- outbound extra_content stripping (wayland-core#120) ---

    #[test]
    fn test_tool_call_extra_content_stripped_for_strict_provider() {
        // wayland-core#120: the internal extra_content blob (captured from an
        // inbound tool_calls[].extra_content, e.g. Gemini's google.thought
        // marker) must NOT be echoed onto outbound tool_calls for providers
        // that reject unknown fields. On long-context replay, strict
        // OpenAI-compat endpoints (Fireworks / GLM-5 via Flux) 400 with
        // "Extra inputs are not permitted ... tool_calls[0].extra_content".
        // An answering ToolResult so tc1 is NOT an orphan: openai_compat() sets
        // clean_orphan_tool_calls, which would otherwise prune the lone tool_call
        // and make the assertions below vacuous. A real long-context replay always
        // carries the matching result, so this mirrors the customer scenario.
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({"cmd": "ls"}),
                    extra: Some(json!({"google": {"thought": true}})),
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];

        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tc = &assistant["tool_calls"][0];

        assert!(
            tc.get("extra_content").is_none(),
            "extra_content must be stripped from outbound tool_calls for strict providers"
        );
        // The tool call itself is otherwise intact.
        assert_eq!(tc["id"], "tc1");
        assert_eq!(tc["function"]["name"], "bash");
    }

    #[test]
    fn test_tool_call_extra_content_preserved_for_gemini() {
        // Google/Gemini's OpenAI-compat endpoint emitted extra_content and
        // tolerates its round-trip, so it (and only it) keeps emitting it.
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tc1".into(),
                name: "bash".into(),
                input: json!({}),
                extra: Some(json!({"google": {"thought": true}})),
            }],
        )];

        let result =
            OpenAIProvider::build_messages(&messages, "", &ProviderCompat::gemini_defaults());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tc = &assistant["tool_calls"][0];

        assert_eq!(tc["extra_content"], json!({"google": {"thought": true}}));
    }

    // --- dedup_tool_results ---

    #[test]
    fn test_dedup_tool_results_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }

    // --- #170: empty tool_call_id guard ---

    #[test]
    fn strips_empty_tool_call_ids_keeps_valid_ones() {
        // An upstream router dropped the id on one tool call (empty id on both
        // the assistant tool_call AND its result). A second exchange is intact.
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(), // upstream-dropped id
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "orphaned".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc_good".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc_good".into(),
                    content: "kept".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());

        // No tool message may carry an empty/missing tool_call_id (would 400).
        for m in &result {
            if m["role"] == "tool" {
                let id = m["tool_call_id"].as_str().unwrap_or("");
                assert!(!id.is_empty(), "empty tool_call_id must be stripped: {m}");
            }
        }
        // No assistant tool_call may carry an empty id either.
        for m in &result {
            if let Some(tcs) = m["tool_calls"].as_array() {
                for tc in tcs {
                    assert!(
                        !tc["id"].as_str().unwrap_or("").is_empty(),
                        "empty tool_call id must be stripped: {tc}"
                    );
                }
            }
        }
        // The intact exchange survives.
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "tc_good");
        assert_eq!(tool_msgs[0]["content"], "kept");
    }

    // --- #123: orphaned tool result hardening ---

    /// `clean_orphaned_tool_results` must drop a `role:"tool"` message whose
    /// `tool_call_id` is unmatched OR missing entirely — the latter is the
    /// "out of scope" gap that previously let a no-id tool message survive to
    /// the provider and 400 DeepSeek via Flux (parity-sweep rank 8).
    #[test]
    fn clean_orphaned_tool_results_strips_missing_and_unmatched_ids() {
        let mut messages = vec![
            // Surviving pair.
            json!({
                "role": "assistant",
                "tool_calls": [{ "id": "tc_ok", "type": "function",
                    "function": { "name": "bash", "arguments": "{}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "tc_ok", "content": "kept" }),
            // Orphan: id present but no matching assistant tool_call.
            json!({ "role": "tool", "tool_call_id": "tc_gone", "content": "unmatched" }),
            // Orphan: no tool_call_id at all (the previously-"out of scope" case).
            json!({ "role": "tool", "content": "no id" }),
        ];

        clean_orphaned_tool_results(&mut messages);

        let tool_msgs: Vec<_> = messages.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1, "only the matched result survives");
        assert_eq!(tool_msgs[0]["tool_call_id"], "tc_ok");
    }

    /// End-to-end contract: a history whose assistant `tool_calls` turn was
    /// trimmed away (leaving the result orphaned) must serialize to ZERO
    /// `role:"tool"` messages lacking a matching `tool_call_id`
    /// (parity-sweep rank 7).
    #[test]
    fn split_tool_pair_serializes_without_orphans() {
        // Only the result survives; its parent assistant tool_use is gone, as
        // a context trim that split the pair would leave it.
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "earlier turn".into(),
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc_orphan".into(),
                    content: "orphaned result".into(),
                    is_error: false,
                }],
            ),
        ];

        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());

        for m in &result {
            if m["role"] == "tool" {
                let id = m["tool_call_id"].as_str().unwrap_or("");
                let matched = result.iter().any(|a| {
                    a["tool_calls"]
                        .as_array()
                        .is_some_and(|tcs| tcs.iter().any(|tc| tc["id"] == id))
                });
                assert!(
                    matched && !id.is_empty(),
                    "orphaned tool result must not reach the provider: {m}"
                );
            }
        }
    }

    /// #123: when the orphan cleanup strips a tool-call-only assistant's only
    /// `tool_calls` entry, the resulting message must still carry `content`
    /// (native DeepSeek 400s on an assistant with neither content nor
    /// tool_calls). Here the assistant's tool call is never answered, so
    /// `clean_orphaned_tool_calls` removes it.
    #[test]
    fn stripped_tool_call_leaves_valid_assistant_content() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "go".into() }]),
            // Text-less assistant turn: a single tool call, no answering result.
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_unanswered".into(),
                    name: "get".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "never answered".into(),
                }],
            ),
        ];

        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());

        for m in &result {
            if m["role"] == "assistant" {
                let has_calls = m["tool_calls"].as_array().is_some_and(|a| !a.is_empty());
                let has_content = m["content"].is_string();
                assert!(
                    has_calls || has_content,
                    "assistant must carry content or tool_calls (would 400 DeepSeek): {m}"
                );
            }
        }
    }

    // --- usage token parsing ---

    #[test]
    fn test_usage_from_trailing_chunk() {
        // OpenAI sends usage in a trailing chunk where choices:[] — the Done
        // event must carry the token counts from that chunk, not zeros.
        let mut state = StreamState::new();

        // chunk 1: finish_reason + text delta, no usage
        let chunk1 = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_chunk(chunk1, &mut state);
        // TextDelta is emitted immediately; Done is deferred.
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred, not emitted with finish_reason chunk"
        );
        assert!(state.pending_done.is_some());

        // chunk 2: trailing usage-only chunk (choices:[])
        let chunk2 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state);
        assert!(events2.is_empty());
        assert_eq!(state.input_tokens, 10);
        assert_eq!(state.output_tokens, 5);

        // [DONE] — flush with final counts
        let done = state.flush_done().expect("pending_done should be Some");
        match done {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(finish_reason, FinishReason::Stop);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_usage_in_finish_chunk() {
        // Some providers/models include usage in the same chunk as finish_reason.
        // Counts should still be correct after flush.
        let mut state = StreamState::new();

        // No text delta here, only finish_reason + usage in the same chunk.
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred even when usage is in the finish chunk"
        );
        assert_eq!(state.output_tokens, 3);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_build_tools_deferred_has_empty_parameters() {
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
        let result = OpenAIProvider::build_tools(&tools);

        // Core tool has full parameters
        let read_params = &result[0]["function"]["parameters"];
        assert!(read_params["properties"].get("path").is_some());

        // Deferred tool has empty parameters and modified description
        let spawn_params = &result[1]["function"]["parameters"];
        assert!(spawn_params["properties"].as_object().unwrap().is_empty());
        let spawn_desc = result[1]["function"]["description"].as_str().unwrap();
        assert!(spawn_desc.contains("ToolSearch"));
    }

    /// #297: WCore tool ids contain `:` / `::` / `.`, which OpenAI rejects with
    /// `400 invalid_value` on `tools[N].function.name`. build_tools must emit
    /// names matching `^[a-zA-Z0-9_-]+$`, and a clean snake_case name must be
    /// left untouched (no blast radius for normal tools).
    #[test]
    fn build_tools_sanitizes_invalid_names_issue_297() {
        let tools = vec![
            ToolDef {
                name: "Browser::execute".into(),
                description: "b".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "ai.perplexity-perplexity-mcp".into(),
                description: "p".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "get_weather".into(),
                description: "w".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
        ];
        let result = OpenAIProvider::build_tools(&tools);
        let wire_legal = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        for entry in &result {
            let name = entry["function"]["name"].as_str().unwrap();
            assert!(wire_legal(name), "emitted illegal wire name: {name}");
        }
        // Invalid names are wrapped; the clean snake_case one is verbatim.
        assert_ne!(result[0]["function"]["name"], json!("Browser::execute"));
        assert_eq!(result[2]["function"]["name"], json!("get_weather"));
        // And they decode back to the canonical ids the engine dispatches on.
        assert_eq!(
            decode_tool_name(result[0]["function"]["name"].as_str().unwrap()),
            "Browser::execute"
        );
        assert_eq!(
            decode_tool_name(result[1]["function"]["name"].as_str().unwrap()),
            "ai.perplexity-perplexity-mcp"
        );
    }

    /// #139: plugin fq_names (`Browser::execute`) must be wire-legal at EVERY
    /// name-emitting site of the full chat request body — the `tools[]`
    /// definitions AND replayed assistant-history `tool_calls`. Flux forwards
    /// both verbatim to strict downstreams (DeepSeek enforces
    /// `^[a-zA-Z0-9_-]+$` and 400s the whole request), and the engine cannot
    /// see past Flux to the real downstream, so encoding must be universal.
    #[test]
    fn build_request_body_emits_wire_legal_names_everywhere_issue_139() {
        let raw = "Browser::execute";
        let mut req = stop_req();
        req.tools = vec![
            ToolDef {
                name: raw.into(),
                description: "run a browser action".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "get_weather".into(),
                description: "w".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
                server: None,
            },
        ];
        req.messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "go".into() }]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc_1".into(),
                    name: raw.into(),
                    input: json!({"cmd": "ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc_1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "and?".into(),
                }],
            ),
        ];

        let body = stop_provider().build_request_body(&req);
        let wire_legal = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };

        // Tool definitions: every emitted name satisfies the strict pattern;
        // the dirty one decodes back to the canonical fq_name.
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2);
        for t in tools {
            let name = t["function"]["name"].as_str().unwrap();
            assert!(
                wire_legal(name),
                "illegal tool-def name on the wire: {name}"
            );
        }
        let wire_name = tools[0]["function"]["name"].as_str().unwrap();
        assert_ne!(wire_name, raw, "fq_name must not leak raw");
        assert_eq!(decode_tool_name(wire_name), raw);
        assert_eq!(tools[1]["function"]["name"], json!("get_weather"));

        // Replayed assistant-history tool_calls carry the SAME encoded
        // spelling (a mismatch would also 400 or desync the transcript).
        let messages = body["messages"].as_array().expect("messages array");
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant" && m.get("tool_calls").is_some())
            .expect("assistant tool_calls message survived history lowering");
        let hist_name = assistant["tool_calls"][0]["function"]["name"]
            .as_str()
            .unwrap();
        assert!(wire_legal(hist_name), "illegal history name: {hist_name}");
        assert_eq!(hist_name, wire_name, "history and tools[] must agree");
    }

    /// #139 inbound half of the round-trip: the model calls back with the
    /// SANITIZED wire name; the chat stream parser must emit
    /// `LlmEvent::ToolUse` carrying the ORIGINAL registry fq_name, or the
    /// engine dispatches into a void ("unknown tool").
    #[test]
    fn streamed_sanitized_tool_call_dispatches_original_name_issue_139() {
        let raw = "Browser::execute";
        let wire = encode_tool_name(raw);
        assert_ne!(wire, raw, "fixture must actually be encoded");

        let mut state = StreamState::new();
        let delta = json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "tc_1",
                        "type": "function",
                        "function": { "name": wire, "arguments": "{\"cmd\":\"ls\"}" }
                    }]
                }
            }]
        })
        .to_string();
        assert!(
            parse_sse_chunk(&delta, &mut state).is_empty(),
            "no events until finish_reason"
        );

        let finish = json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        })
        .to_string();
        let events = parse_sse_chunk(&finish, &mut state);
        let (name, input) = events
            .iter()
            .find_map(|e| match e {
                LlmEvent::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
                _ => None,
            })
            .expect("ToolUse event on finish");
        assert_eq!(
            name, raw,
            "inbound tool_call must decode to the canonical fq_name"
        );
        assert_eq!(input, json!({"cmd": "ls"}));
    }

    #[test]
    fn test_build_tools_normalizes_bare_object_schema_issue_24() {
        // Built-in and MCP tools that declare no structured args carry a bare
        // `{"type":"object"}`. Strict OpenAI servers (LM Studio) reject that for
        // missing `properties`. build_tools must normalize it. (#24)
        let tools = vec![
            ToolDef {
                name: "execute".into(),
                description: "run a command".into(),
                input_schema: json!({"type": "object"}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "ijfw_run".into(),
                description: "ijfw".into(),
                // arbitrary-object tool, still missing properties
                input_schema: json!({"type": "object", "additionalProperties": true}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "Read".into(),
                description: "read".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
                deferred: false,
                server: None,
            },
        ];
        let result = OpenAIProvider::build_tools(&tools);

        // bare {type:object} -> gains properties:{} and required:[]
        let exec = &result[0]["function"]["parameters"];
        assert_eq!(exec["type"], "object");
        assert!(exec["properties"].is_object());
        assert!(exec["properties"].as_object().unwrap().is_empty());
        assert!(exec["required"].is_array());

        // additionalProperties is preserved; properties + required still added
        let ijfw = &result[1]["function"]["parameters"];
        assert!(ijfw["properties"].is_object());
        assert_eq!(ijfw["additionalProperties"], json!(true));
        assert!(ijfw["required"].is_array());

        // a well-formed schema is passed through untouched
        let read = &result[2]["function"]["parameters"];
        assert!(read["properties"].get("path").is_some());
        assert_eq!(read["required"], json!(["path"]));
    }

    #[test]
    fn usage_includes_prompt_cache_hit_tokens() {
        // DeepSeek reports prompt_cache_hit_tokens separately;
        // input_tokens should be the sum of prompt_tokens + prompt_cache_hit_tokens
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":500,"completion_tokens":100,"prompt_cache_hit_tokens":999500}}"#;
        let _ = parse_sse_chunk(chunk, &mut state);

        assert_eq!(state.input_tokens, 1_000_000);
        assert_eq!(state.output_tokens, 100);
    }

    #[test]
    fn usage_with_prompt_tokens_details_cached() {
        // OpenAI standard: prompt_tokens already includes cached_tokens (it's the total)
        // prompt_tokens_details.cached_tokens is informational only
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":999000}}}"#;
        let _ = parse_sse_chunk(chunk, &mut state);

        // prompt_tokens is already the full total for OpenAI
        assert_eq!(state.input_tokens, 1_000_000);
        assert_eq!(state.output_tokens, 100);
    }

    #[test]
    fn chat_path_emits_cache_read_tokens_in_done() {
        // Rank 39: the chat path must surface cache-read tokens in the Done
        // event (like the Responses path), not hardcode 0. OpenAI-standard
        // reports the hit count under prompt_tokens_details.cached_tokens while
        // prompt_tokens stays the full total — so input_tokens is unchanged.
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":800}}}"#;
        let _ = parse_sse_chunk(chunk, &mut state);

        assert_eq!(state.input_tokens, 1000, "prompt_tokens stays the total");
        assert_eq!(state.cache_read_tokens, 800);

        let done = state.flush_done().expect("pending_done should be Some");
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.cache_read_tokens, 800);
                assert_eq!(usage.input_tokens, 1000);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn chat_path_cache_read_from_deepseek_hit_field() {
        // DeepSeek reports the hit count separately and prompt_tokens carries
        // only the cache-miss portion; cache_read_tokens must reflect the hit.
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":200,"completion_tokens":10,"prompt_cache_hit_tokens":800}}"#;
        let _ = parse_sse_chunk(chunk, &mut state);

        assert_eq!(state.input_tokens, 1000, "miss + hit = total prompt");
        assert_eq!(state.cache_read_tokens, 800);
    }

    #[test]
    fn usage_without_cache_fields_unchanged() {
        // Provider that only sends prompt_tokens (no cache fields)
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50000,"completion_tokens":200}}"#;
        let _ = parse_sse_chunk(chunk, &mut state);

        assert_eq!(state.input_tokens, 50_000);
        assert_eq!(state.output_tokens, 200);
    }

    #[test]
    fn tool_calls_with_stop_finish_reason() {
        // Gemini uses finish_reason:"stop" even when tool_calls are present.
        // The accumulated tool calls must still be emitted.
        let mut state = StreamState::new();

        // chunk 1: tool call delta (name + partial args)
        let chunk1 = r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"extra_content":{},"function":{"arguments":"{\"skill\":\"test\",\"args\":\"hello\"}","name":"Skill"},"id":"call_abc123","type":"function"}]},"index":0}]}"#;
        let events1 = parse_sse_chunk(chunk1, &mut state);
        assert!(events1.is_empty(), "no events until finish_reason");
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0].name, "Skill");

        // chunk 2: finish_reason:"stop" (not "tool_calls")
        let chunk2 = r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state);

        // Tool call should be emitted
        let tool_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .collect();
        assert_eq!(tool_events.len(), 1, "tool call should be emitted on stop");
        if let LlmEvent::ToolUse {
            id, name, input, ..
        } = &tool_events[0]
        {
            assert_eq!(id, "call_abc123");
            assert_eq!(name, "Skill");
            assert_eq!(input["skill"], "test");
        }

        // Done should be deferred with ToolUse stop reason
        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done with ToolUse, got {other:?}"),
        }

        assert!(state.tool_calls.is_empty(), "tool calls should be drained");
    }

    #[test]
    fn stop_without_tool_calls_unchanged() {
        // Standard stop without tool calls should still produce EndTurn.
        let mut state = StreamState::new();

        let chunk =
            r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);

        let text_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, LlmEvent::TextDelta(_)))
            .collect();
        assert_eq!(text_events.len(), 1);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            other => panic!("expected Done with EndTurn, got {other:?}"),
        }
    }

    // --- B.3 Gemini reasoning-token routing -------------------------------

    /// `delta.reasoning_content` (OpenAI o-series and Gemini Pro on the
    /// v1beta openai-compat surface) must become `LlmEvent::ThinkingDelta`,
    /// never folded into `TextDelta`. Regression for the long-form Gemini
    /// reasoning bug where thoughts were dropped into the text channel.
    #[test]
    fn test_gemini_reasoning_content_routes_to_thinking_delta() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"reasoning_content":"step 1: parse"},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1, "expected exactly one ThinkingDelta");
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "step 1: parse"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    /// Some Gemini-served openai-compat variants drop the `_content` suffix
    /// and emit `delta.reasoning` (plain). Treat it identically — otherwise
    /// thoughts silently disappear on that codepath.
    #[test]
    fn test_gemini_reasoning_plain_field_routes_to_thinking_delta() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"reasoning":"step 2: lookup"},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1, "expected exactly one ThinkingDelta");
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "step 2: lookup"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    /// Gemini flags reasoning chunks via
    /// `delta.extra_content.google.thought == true` on what is otherwise a
    /// normal `delta.content` text chunk. Without the B.3 fix these chunks
    /// were routed to `TextDelta`, leaking thoughts into the visible
    /// transcript and producing the "duplicated character" symptom when
    /// the host later rendered the resolved answer.
    #[test]
    fn test_gemini_extra_content_thought_routes_to_thinking_delta() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"content":"weighing options","extra_content":{"google":{"thought":true}}},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1, "expected exactly one ThinkingDelta");
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "weighing options"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    // --- #318 Flux reasoning_summary → thinking SUBJECT ------------------

    /// #318: a chunk carrying only `delta.reasoning_summary` (Flux's per-turn
    /// opaque subject label, emitted just before the first reasoning_content)
    /// must yield exactly one `LlmEvent::ThinkingSubject`, NOT a ThinkingDelta
    /// or TextDelta. Mirrors `parse_reasoning_summary_delta_is_thinking` on
    /// the Responses-API path.
    #[test]
    fn parse_reasoning_summary_delta_is_thinking_subject() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"reasoning_summary":"Reasoning through the problem"},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1, "expected exactly one ThinkingSubject");
        match &events[0] {
            LlmEvent::ThinkingSubject(s) => assert_eq!(s, "Reasoning through the problem"),
            other => panic!("expected ThinkingSubject, got {other:?}"),
        }
    }

    /// #318: a normal `delta.reasoning_content` chunk (no reasoning_summary)
    /// still yields `ThinkingDelta` — the subject path is additive and must
    /// not disturb the established reasoning-text routing.
    #[test]
    fn reasoning_content_without_summary_still_thinking_delta() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"reasoning_content":"step 1: parse"},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "step 1: parse"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    /// #318: an absent `reasoning_summary` field is a clean no-op — a plain
    /// text chunk produces only its `TextDelta`, never a stray subject.
    #[test]
    fn absent_reasoning_summary_is_noop() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"content":"hello"},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LlmEvent::ThinkingSubject(_))),
            "absent reasoning_summary must not synthesize a subject, got {events:?}"
        );
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    /// `extra_content.google.thought == false` (or absent) must still
    /// route `delta.content` to `TextDelta` — the B.3 fix is opt-in on the
    /// flag, never a blanket reroute.
    #[test]
    fn test_gemini_extra_content_thought_false_stays_text() {
        let mut state = StreamState::new();
        let chunk = r#"{"choices":[{"delta":{"content":"final answer","extra_content":{"google":{"thought":false}}},"index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "final answer"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    /// Multi-chunk Gemini stream: one thought chunk + one text chunk must
    /// produce one `ThinkingDelta` + one `TextDelta`, with no characters
    /// dropped or duplicated across the two channels.
    #[test]
    fn test_gemini_thought_then_text_no_drop_no_dup() {
        let mut state = StreamState::new();

        let thought_chunk =
            r#"{"choices":[{"delta":{"reasoning_content":"thinking..."},"index":0}]}"#;
        let text_chunk = r#"{"choices":[{"delta":{"content":"answer"},"index":0}]}"#;

        let mut all_events = parse_sse_chunk(thought_chunk, &mut state);
        all_events.extend(parse_sse_chunk(text_chunk, &mut state));

        let thinking: Vec<&str> = all_events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::ThinkingDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        let text: Vec<&str> = all_events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(thinking.concat(), "thinking...");
        assert_eq!(text.concat(), "answer");
        // No accidental duplication into the other channel.
        assert_eq!(thinking.join("").matches("answer").count(), 0);
        assert_eq!(text.join("").matches("thinking").count(), 0);
    }

    // --- multi-key rotation (covers OpenAI + every openai-compat newtype) ----

    /// The selected key authenticates the request via `Authorization: Bearer`.
    #[test]
    fn build_headers_uses_selected_key_as_bearer() {
        let provider = OpenAIProvider::new(
            "explicit-key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let key = provider.select_key().expect("single key selects");
        let headers = provider.build_headers(&key).expect("headers build");
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer explicit-key"),
        );
    }

    // --- keyless self-hosted endpoints (#398: "OpenAI API key is required"
    //     on a local Ollama model) -------------------------------------------

    #[test]
    fn self_hosted_base_urls_are_detected() {
        for url in [
            "http://localhost:11434/v1",
            "http://localhost",
            "http://127.0.0.1:11434/v1",
            "http://0.0.0.0:8080",
            "http://[::1]:11434/v1",
            "http://host.docker.internal:11434/v1",
            "http://ollama.lan.local:11434",
            "http://10.0.0.5:11434/v1",
            "http://192.168.1.50:11434",
            "http://172.16.4.4:11434",
            "http://172.31.255.1:11434",
            "http://100.109.207.54:11434/v1", // Tailscale CGNAT
        ] {
            assert!(is_self_hosted_base_url(url), "expected self-hosted: {url}");
        }
    }

    #[test]
    fn public_base_urls_are_not_self_hosted() {
        for url in [
            "https://api.openai.com/v1",
            "https://generativelanguage.googleapis.com",
            "https://api.x.ai/v1",
            "http://8.8.8.8/v1",
            "http://172.32.0.1:11434",  // just outside 172.16/12
            "http://100.200.0.1:11434", // just outside 100.64/10
            "",
        ] {
            assert!(!is_self_hosted_base_url(url), "expected public: {url}");
        }
    }

    /// A keyless local endpoint (e.g. Ollama over its OpenAI-compatible surface)
    /// must NOT fail with `MissingApiKey`; it gets the benign placeholder bearer
    /// so the turn reaches the local server, which ignores the header.
    #[test]
    fn keyless_self_hosted_endpoint_uses_placeholder_bearer() {
        let provider = OpenAIProvider::new(
            "",
            "http://localhost:11434/v1",
            openai_compat(),
            DebugConfig::default(),
        );
        assert_eq!(
            provider
                .select_key()
                .expect("self-hosted keyless selects placeholder"),
            SELF_HOSTED_PLACEHOLDER_KEY,
        );
    }

    /// A keyless PUBLIC endpoint still surfaces the clear missing-key error, so
    /// real cloud providers don't silently send a bogus bearer.
    #[test]
    fn keyless_public_endpoint_still_errors_missing_key() {
        let provider = OpenAIProvider::new(
            "",
            "https://api.openai.com/v1",
            openai_compat(),
            DebugConfig::default(),
        );
        assert!(matches!(
            provider.select_key(),
            Err(ProviderError::MissingApiKey)
        ));
    }

    /// Multi-key rotation: after the current key is demoted via
    /// `mark_key_failure`, `select_key` rotates to a different key; once the
    /// new key succeeds it becomes sticky.
    #[test]
    fn multi_key_rotation_demotes_failing_key_then_succeeds() {
        let provider = OpenAIProvider::new(
            "key-a, key-b",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        let first = provider.select_key().expect("a key is available");
        assert!(first == "key-a" || first == "key-b");

        provider.mark_key_failure(&first);
        let second = provider.select_key().expect("rotation finds the other key");
        assert_ne!(second, first, "failing key must rotate to the other key");

        provider.mark_key_success(&second);
        assert_eq!(
            provider.select_key().expect("sticky key"),
            second,
            "a succeeded key must stick as last-good"
        );
    }

    /// Single-key behavior is unchanged: `select_key` always returns the one
    /// configured key. No key configured surfaces `MissingApiKey`.
    #[test]
    fn single_key_behavior_unchanged_and_empty_errors() {
        let provider = OpenAIProvider::new(
            "solo-key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
        );
        assert_eq!(provider.select_key().unwrap(), "solo-key");
        provider.mark_key_success("solo-key");
        assert_eq!(provider.select_key().unwrap(), "solo-key");

        // No key on a PUBLIC endpoint still surfaces MissingApiKey. (A
        // self-hosted endpoint is deliberately keyless — see
        // `keyless_self_hosted_endpoint_uses_placeholder_bearer`.)
        let empty = OpenAIProvider::new(
            "",
            "https://api.openai.com/v1",
            openai_compat(),
            DebugConfig::default(),
        );
        assert!(matches!(
            empty.select_key(),
            Err(ProviderError::MissingApiKey)
        ));
    }
}
