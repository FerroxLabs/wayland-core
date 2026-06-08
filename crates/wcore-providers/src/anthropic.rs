use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};

use super::anthropic_shared;
use crate::cache_tier::CacheTier;
use crate::retry::builder_send_with_retry;
use crate::{
    LlmProvider, ModelInfo, ProviderError, alias_models, dump_request_body, reset_response_dump,
};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

/// The resolved request credential. Anthropic console API keys authenticate
/// via the `x-api-key` header.
enum Credential {
    ApiKey(String),
}

pub struct AnthropicProvider {
    client: wcore_egress::EgressClient,
    api_key: String,
    base_url: String,
    cache_enabled: bool,
    compat: ProviderCompat,
    debug: DebugConfig,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            client: crate::http_client::build(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            cache_enabled: true,
            compat,
            debug,
        }
    }

    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self
    }

    /// Resolve the credential that authenticates the outgoing request.
    /// Anthropic ships with API-key (`x-api-key`) auth only: a configured
    /// API key authenticates the request; when none is set this returns
    /// [`ProviderError::MissingApiKey`] rather than a blank credential that
    /// would produce an opaque 401.
    async fn resolve_credential(&self) -> Result<Credential, ProviderError> {
        if !self.api_key.is_empty() {
            return Ok(Credential::ApiKey(self.api_key.clone()));
        }
        Err(ProviderError::MissingApiKey)
    }

    async fn build_headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let credential = self.resolve_credential().await?;

        match &credential {
            // Console API key: Anthropic authenticates these via `x-api-key`.
            Credential::ApiKey(key) => {
                let value = HeaderValue::from_str(key).map_err(|e| {
                    ProviderError::Connection(format!("Invalid x-api-key header: {}", e))
                })?;
                headers.insert("x-api-key", value);
            }
        }

        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.cache_enabled {
            // Prompt-caching beta opt-in. Unrelated to credential auth.
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static("prompt-caching-2024-07-31"),
            );
        }
        Ok(headers)
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        // Always start by building the system block as a structured array so the
        // 3-zone cache injector has a uniform shape to mark. When caching is
        // disabled we still emit the structured form when system is non-empty,
        // which Anthropic accepts.
        let system = if request.system.is_empty() {
            json!([])
        } else {
            json!([{ "type": "text", "text": &request.system }])
        };

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat),
            "stream": true
        });

        if !request.tools.is_empty() {
            body["tools"] = json!(anthropic_shared::build_tools(&request.tools));
        }

        if let Some(ThinkingConfig::Enabled { budget_tokens }) = &request.thinking {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
        }

        // Inject 3-zone cache_control markers (system / tools / messages) when
        // caching is enabled. W8 v0.6.3: honor `request.cache_tier` when the
        // caller selected one (e.g. via `cache_tier::pick_cache_tier`), falling
        // back to the 5m default when unset.
        if self.cache_enabled {
            apply_cache_zones(
                &mut body,
                request.cache_tier.unwrap_or(CacheTier::Ephemeral5m),
            );
        }

        body
    }
}

/// Inject `cache_control` markers at the 3 standard Anthropic prompt-cache
/// zones: the last `system` block, the last `tools` entry, and the last
/// `messages` entry. Idempotent — re-running on the same body produces the
/// same output. Skips zones cleanly when empty (no system text / no tools /
/// no messages).
///
/// `tier` controls the ephemeral TTL marker:
///   - [`CacheTier::Ephemeral5m`] -> `{ "type": "ephemeral" }` (default 5m)
///   - [`CacheTier::Ephemeral1h`] -> `{ "type": "ephemeral", "ttl": "1h" }`
///   - [`CacheTier::None`]        -> no-op (caching disabled for this request)
pub fn apply_cache_zones(body: &mut Value, tier: CacheTier) {
    let marker = match cache_control_marker(tier) {
        Some(m) => m,
        None => return,
    };

    // Zone 1: system prompt — mark the last text block.
    if let Some(system_blocks) = body.get_mut("system").and_then(Value::as_array_mut)
        && let Some(last) = system_blocks.last_mut()
        && last.is_object()
    {
        last["cache_control"] = marker.clone();
    }

    // Zone 2: tools — mark the last tool definition.
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut)
        && let Some(last) = tools.last_mut()
        && last.is_object()
    {
        last["cache_control"] = marker.clone();
    }

    // Zone 3: messages — mark the last message so the running context up to
    // and including that message becomes a cache prefix for the next turn.
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut)
        && let Some(last) = messages.last_mut()
    {
        apply_message_zone_marker(last, &marker);
    }
}

/// Attach the cache_control marker to a single message. Prefers the last
/// content block (the natural cache boundary for tool_result / text content);
/// falls back to a top-level marker when content is a bare string or absent.
fn apply_message_zone_marker(message: &mut Value, marker: &Value) {
    if let Some(content) = message.get_mut("content")
        && let Some(blocks) = content.as_array_mut()
        && let Some(last_block) = blocks.last_mut()
        && last_block.is_object()
    {
        last_block["cache_control"] = marker.clone();
        return;
    }
    if message.is_object() {
        message["cache_control"] = marker.clone();
    }
}

/// Build the `cache_control` marker JSON for a given tier, or `None` when the
/// tier is [`CacheTier::None`].
fn cache_control_marker(tier: CacheTier) -> Option<Value> {
    match tier {
        CacheTier::Ephemeral5m => Some(json!({ "type": "ephemeral" })),
        CacheTier::Ephemeral1h => Some(json!({ "type": "ephemeral", "ttl": "1h" })),
        CacheTier::None => None,
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let response = builder_send_with_retry(
            self.client
                .post(&url)
                .headers(self.build_headers().await?)
                .json(&body),
        )
        .await?;

        let status = response.status();
        if !status.is_success() {
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

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        tokio::spawn(async move {
            if let Err(e) = anthropic_shared::process_sse_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }

    fn alias_key(&self) -> &str {
        "anthropic"
    }

    /// Live model discovery via Anthropic's `GET /v1/models` endpoint. On any
    /// HTTP/parse failure we fall back to the static alias catalog — `/model`
    /// must never hard-fail.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let headers = match self.build_headers().await {
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
            parse_anthropic_models(&body)
        }
        .await;

        match live {
            Ok(models) if !models.is_empty() => Ok(models),
            _ => Ok(alias_models(self.alias_key())),
        }
    }
}

/// Parse an Anthropic `GET /v1/models` response body into [`ModelInfo`]s.
/// The documented shape is
/// `{"data":[{"id":"claude-...","display_name":"Claude ...","type":"model"}]}`.
/// `id` is required; `display_name` is used as the label when present,
/// otherwise the label mirrors the id. Entries without a non-empty string
/// `id` are skipped.
fn parse_anthropic_models(body: &str) -> anyhow::Result<Vec<ModelInfo>> {
    let json: Value = serde_json::from_str(body)?;
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("models response missing `data` array"))?;
    let models = data
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let display = entry
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .unwrap_or(id);
            Some(ModelInfo {
                id: id.to_string(),
                display: display.to_string(),
            })
        })
        .collect();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_5m() -> Value {
        json!({ "type": "ephemeral" })
    }

    // --- D008: live /model library — /v1/models parse --------------------

    #[test]
    fn parse_anthropic_models_uses_display_name_when_present() {
        let body = r#"{"data":[
            {"type":"model","id":"claude-opus-4-6","display_name":"Claude Opus 4.6"},
            {"type":"model","id":"claude-sonnet-4-6","display_name":"Claude Sonnet 4.6"}
        ]}"#;
        let models = parse_anthropic_models(body).expect("valid body parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-opus-4-6");
        assert_eq!(models[0].display, "Claude Opus 4.6");
    }

    #[test]
    fn parse_anthropic_models_falls_back_to_id_when_no_display_name() {
        let body = r#"{"data":[{"type":"model","id":"claude-haiku-4-5"}]}"#;
        let models = parse_anthropic_models(body).expect("parses");
        assert_eq!(models.len(), 1);
        // No display_name → label mirrors the id.
        assert_eq!(models[0].display, "claude-haiku-4-5");
    }

    #[test]
    fn parse_anthropic_models_skips_invalid_ids_and_errors_on_no_data() {
        let body = r#"{"data":[{"id":""},{"display_name":"x"},{"id":"claude-opus-4-6"}]}"#;
        let models = parse_anthropic_models(body).expect("parses");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-opus-4-6");

        // Missing `data` array → Err so the caller uses the alias fallback.
        assert!(parse_anthropic_models(r#"{"error":"nope"}"#).is_err());
        assert!(parse_anthropic_models("garbage").is_err());
    }

    /// A configured API key authenticates the request via the `x-api-key`
    /// header — Anthropic ships with API-key auth only.
    #[tokio::test]
    async fn build_headers_sends_api_key_as_x_api_key() {
        let provider = AnthropicProvider::new(
            "explicit-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let headers = provider
            .build_headers()
            .await
            .expect("build_headers must succeed with an api key");
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("explicit-key"),
            "a configured api key must go on the wire as x-api-key"
        );
        assert!(
            headers.get(reqwest::header::AUTHORIZATION).is_none(),
            "Anthropic api-key auth must not emit an Authorization header"
        );
    }

    /// With NO api key, `build_headers` must fail with `MissingApiKey` rather
    /// than emit a blank credential producing an opaque 401.
    #[tokio::test]
    async fn build_headers_errors_when_no_api_key() {
        let provider = AnthropicProvider::new(
            "",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let result = provider.build_headers().await;
        assert!(
            matches!(result, Err(ProviderError::MissingApiKey)),
            "no api key must surface MissingApiKey, got {result:?}"
        );
    }

    fn marker_1h() -> Value {
        json!({ "type": "ephemeral", "ttl": "1h" })
    }

    fn body_with_all_zones() -> Value {
        json!({
            "model": "claude-3-5-sonnet",
            "system": [
                { "type": "text", "text": "first system block" },
                { "type": "text", "text": "second system block" }
            ],
            "tools": [
                { "name": "tool_a", "description": "a" },
                { "name": "tool_b", "description": "b" }
            ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "hi" } ] },
                { "role": "assistant", "content": [ { "type": "text", "text": "hello" } ] }
            ]
        })
    }

    #[test]
    fn all_three_zones_injected_when_present() {
        let mut body = body_with_all_zones();
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        // System: last block has marker, first does not.
        let system = body["system"].as_array().unwrap();
        assert!(system[0].get("cache_control").is_none());
        assert_eq!(system[1]["cache_control"], marker_5m());

        // Tools: last tool has marker, first does not.
        let tools = body["tools"].as_array().unwrap();
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["cache_control"], marker_5m());

        // Messages: last message's last content block has marker.
        let messages = body["messages"].as_array().unwrap();
        assert!(messages[0]["content"][0].get("cache_control").is_none());
        assert_eq!(messages[1]["content"][0]["cache_control"], marker_5m());
    }

    #[test]
    fn system_only_no_tools_marks_system_and_messages_only() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "sys" } ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "hi" } ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        assert_eq!(body["system"][0]["cache_control"], marker_5m());
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            marker_5m()
        );
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tools_only_empty_system_marks_tools_and_messages() {
        let mut body = json!({
            "system": [],
            "tools": [ { "name": "t", "description": "d" } ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "hi" } ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        // Empty system array -> nothing to mark, no panic.
        assert!(body["system"].as_array().unwrap().is_empty());
        assert_eq!(body["tools"][0]["cache_control"], marker_5m());
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            marker_5m()
        );
    }

    #[test]
    fn messages_only_edge_case() {
        let mut body = json!({
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "hi" } ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            marker_5m()
        );
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn idempotent_when_applied_twice() {
        let mut body = body_with_all_zones();
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);
        let after_first = body.clone();
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);
        assert_eq!(body, after_first, "second apply must be a no-op shape");
    }

    #[test]
    fn ttl_propagates_5m_vs_1h() {
        let mut body_5m = body_with_all_zones();
        apply_cache_zones(&mut body_5m, CacheTier::Ephemeral5m);
        assert_eq!(body_5m["system"][1]["cache_control"], marker_5m());
        assert!(body_5m["system"][1]["cache_control"].get("ttl").is_none());

        let mut body_1h = body_with_all_zones();
        apply_cache_zones(&mut body_1h, CacheTier::Ephemeral1h);
        assert_eq!(body_1h["system"][1]["cache_control"], marker_1h());
        assert_eq!(body_1h["tools"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(
            body_1h["messages"][1]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }

    #[test]
    fn cache_tier_none_is_noop() {
        let mut body = body_with_all_zones();
        let before = body.clone();
        apply_cache_zones(&mut body, CacheTier::None);
        assert_eq!(body, before);
    }

    #[test]
    fn snapshot_minimal_body_shape() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "S" } ],
            "tools": [ { "name": "T", "description": "d" } ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "U" } ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        let expected = json!({
            "system": [
                { "type": "text", "text": "S", "cache_control": { "type": "ephemeral" } }
            ],
            "tools": [
                { "name": "T", "description": "d", "cache_control": { "type": "ephemeral" } }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "U", "cache_control": { "type": "ephemeral" } }
                    ]
                }
            ]
        });
        assert_eq!(body, expected);
    }

    #[test]
    fn empty_messages_list_no_message_marker() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "S" } ],
            "messages": []
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);

        // System still marked.
        assert_eq!(body["system"][0]["cache_control"], marker_5m());
        // Messages array stays empty — no marker injected anywhere there.
        assert!(body["messages"].as_array().unwrap().is_empty());
    }

    #[test]
    fn message_with_string_content_falls_back_to_top_level_marker() {
        // Defensive: if a caller hands us a message whose content is a bare
        // string (not a blocks array), we still attach the marker at the
        // message level rather than panicking or silently dropping it.
        let mut body = json!({
            "messages": [
                { "role": "user", "content": "plain string" }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m);
        assert_eq!(body["messages"][0]["cache_control"], marker_5m());
    }

    // --- W8 v0.6.3: build_request_body consumes request.cache_tier ---------

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
    }

    fn cache_req(tier: Option<CacheTier>) -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: "you are a test".into(),
            messages: vec![wcore_types::message::Message::new(
                wcore_types::message::Role::User,
                vec![wcore_types::message::ContentBlock::Text { text: "hi".into() }],
            )],
            tools: vec![],
            max_tokens: 16,
            thinking: None,
            reasoning_effort: None,
            cache_tier: tier,
            routing_hint: None,
        }
    }

    #[test]
    fn build_request_body_uses_request_cache_tier_when_set() {
        // cache_tier = Some(Ephemeral1h) MUST produce the 1h ttl marker.
        let body = provider().build_request_body(&cache_req(Some(CacheTier::Ephemeral1h)));
        assert_eq!(
            body["system"][0]["cache_control"],
            marker_1h(),
            "request.cache_tier=Ephemeral1h must yield the 1h ttl marker"
        );
    }

    #[test]
    fn build_request_body_defaults_to_5m_when_cache_tier_none() {
        // cache_tier = None falls back to the historical Ephemeral5m default.
        let body = provider().build_request_body(&cache_req(None));
        assert_eq!(
            body["system"][0]["cache_control"],
            marker_5m(),
            "request.cache_tier=None must fall back to the 5m default"
        );
    }

    #[test]
    fn build_request_body_cache_tier_none_variant_disables_caching() {
        // cache_tier = Some(CacheTier::None) explicitly disables cache markers.
        let body = provider().build_request_body(&cache_req(Some(CacheTier::None)));
        assert!(
            body["system"][0].get("cache_control").is_none(),
            "request.cache_tier=Some(None) must inject no cache_control marker"
        );
    }
}
