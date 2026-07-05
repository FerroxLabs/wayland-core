use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};

use super::anthropic_shared;
use crate::cache_tier::CacheTier;
use crate::key_rotation::{KeyPool, split_keys};
use crate::retry::builder_send_with_retry;
use crate::{
    LlmProvider, ModelInfo, ProviderError, alias_models, dump_request_body, reset_response_dump,
};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

pub struct AnthropicProvider {
    client: wcore_egress::EgressClient,
    /// Rotation pool over one-or-more API keys. A single configured key yields
    /// a one-element pool — behavior identical to the pre-rotation path. Wrapped
    /// in `Arc<Mutex<…>>` so `&self` request methods can rotate/demote keys.
    keys: Arc<Mutex<KeyPool>>,
    base_url: String,
    cache_enabled: bool,
    /// Breakpoint floor: skip `cache_control` injection when the estimated
    /// prompt prefix is below this many tokens (see `apply_cache_zones`).
    /// From `[providers.anthropic.prompt_caching] min_prefix_tokens`;
    /// defaults to `wcore_config::config::DEFAULT_CACHE_MIN_PREFIX_TOKENS`.
    min_prefix_tokens: usize,
    /// Provider key used to look up the offline `/model` fallback catalog
    /// (`wcore_types::model_aliases::models_for_provider`) when live model
    /// discovery fails. Defaults to `"anthropic"`; Anthropic-wire third parties
    /// that reuse this provider (e.g. MiniMax) set their own key via
    /// [`with_alias_key`] so the picker never falls back to Claude models.
    alias_key: String,
    compat: ProviderCompat,
    debug: DebugConfig,
    /// Once a `compat.auth_fallback_base_url` retry authenticates (region-locked
    /// key failover), the working host is pinned here so every subsequent
    /// request tries it first instead of re-paying the primary's 401. `None`
    /// until a fallback has succeeded.
    pinned_base_url: Arc<Mutex<Option<String>>>,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            client: crate::http_client::build(),
            keys: Arc::new(Mutex::new(KeyPool::new(split_keys(api_key)))),
            base_url: base_url.to_string(),
            cache_enabled: true,
            min_prefix_tokens: wcore_config::config::DEFAULT_CACHE_MIN_PREFIX_TOKENS,
            alias_key: "anthropic".to_string(),
            compat,
            debug,
            pinned_base_url: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self
    }

    /// Override the breakpoint floor (`min_prefix_tokens`). `0` disables the
    /// floor — every cache-enabled request gets breakpoint markers.
    pub fn with_min_prefix_tokens(mut self, min_prefix_tokens: usize) -> Self {
        self.min_prefix_tokens = min_prefix_tokens;
        self
    }

    /// Override the offline model-fallback catalog key (default `"anthropic"`).
    /// Used by Anthropic-wire third parties (e.g. MiniMax) that reuse this
    /// provider but must surface their own models — not Claude — when live
    /// `GET /v1/models` discovery is unavailable.
    pub fn with_alias_key(mut self, key: &str) -> Self {
        self.alias_key = key.to_string();
        self
    }

    /// Select the API key to authenticate the next request. Delegates to
    /// [`KeyPool::next_key`] (prefers the last-good key, rotates round-robin on
    /// failure, skips keys in cooldown). Returns [`ProviderError::MissingApiKey`]
    /// when no key is configured or every key is cooling — matching the
    /// pre-rotation behavior of surfacing a clear error rather than a blank
    /// credential that would produce an opaque 401.
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

    /// The host to try first: a fallback that previously authenticated this
    /// session (region-locked-key failover) if one is pinned, else the
    /// configured primary `base_url`.
    fn effective_base_url(&self) -> String {
        self.pinned_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| self.base_url.clone())
    }

    /// Remember the host that authenticated so subsequent requests skip the
    /// primary's certain 401.
    fn pin_base_url(&self, url: &str) {
        *self
            .pinned_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(url.to_string());
    }

    /// Send one streaming request to a specific `base_url`. Returns the event
    /// receiver on 2xx, or the mapped [`ProviderError`] on any failure. Region
    /// failover (retrying an alternate host) is the caller's concern — this
    /// method targets exactly the host it is given.
    async fn try_stream(
        &self,
        base_url: &str,
        key: &str,
        body: &Value,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}/v1/messages", base_url);
        let response = builder_send_with_retry(
            self.client
                .post(&url)
                .headers(self.build_headers(key)?)
                .json(body),
        )
        .await?;

        let status = response.status();
        if !status.is_success() {
            // Demote this key on auth / rate-limit failures so the next request
            // rotates to another key in the pool (no-op for a single key).
            if matches!(status.as_u16(), 401 | 403 | 429) {
                self.mark_key_failure(key);
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
        self.mark_key_success(key);

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        tokio::spawn(async move {
            if let Err(e) = anthropic_shared::process_sse_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }

    /// Build request headers authenticating with the supplied `key`. Anthropic
    /// console API keys authenticate via the `x-api-key` header.
    fn build_headers(&self, key: &str) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(key)
            .map_err(|e| ProviderError::Connection(format!("Invalid x-api-key header: {}", e)))?;
        headers.insert("x-api-key", value);

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

        // Crucible #3: emit an explicit `temperature` when set, gated by the
        // provider's `supports_temperature` flag + the per-model exclusion (see
        // `openai_compat::emit_temperature`). Anthropic accepts `temperature`.
        crate::openai_compat::emit_temperature(&mut body, request, &self.compat);

        // Output-side optimization (Part A): UNION the request's fluff stop
        // sequences into Anthropic's `stop_sequences` field, preserving any the
        // caller already placed on the body. Must run BEFORE apply_cache_zones
        // so the body shape is final before cache markers are injected. Skipped
        // entirely when empty so back-compatible callers emit no stop field.
        if !request.stop_sequences.is_empty() {
            let stops = body.get_mut("stop_sequences").and_then(Value::as_array_mut);
            match stops {
                Some(existing) => {
                    for s in &request.stop_sequences {
                        existing.push(json!(s));
                    }
                }
                None => {
                    body["stop_sequences"] = json!(request.stop_sequences);
                }
            }
        }

        // Inject 3-zone cache_control markers (system / tools / messages) when
        // caching is enabled. W8 v0.6.3: honor `request.cache_tier` when the
        // caller selected one (e.g. via `cache_tier::pick_cache_tier`), falling
        // back to the 5m default when unset.
        if self.cache_enabled {
            apply_cache_zones(
                &mut body,
                request.cache_tier.unwrap_or(CacheTier::Ephemeral5m),
                self.min_prefix_tokens,
            );
        } else {
            // Codex audit (e399feb1 finding 1): `build_messages` above runs
            // BEFORE this gate and — for anthropic-family compat — has
            // already translated the engine's tail cache hint
            // (`mark_cache_boundaries`) into a wire `cache_control` marker.
            // `prompt_caching = false` must mean a body byte-identical to an
            // uninjected request, so strip that leak here.
            strip_message_cache_markers(&mut body);
        }

        body
    }
}

/// Anthropic's hard limit on `cache_control` blocks per request.
pub const ANTHROPIC_CACHE_CONTROL_LIMIT: usize = 4;

/// Rough bytes-per-token divisor for the `min_prefix_tokens` floor estimate.
/// Deliberately 2 — NOT the usual ~4 bytes/token English heuristic — because
/// the floor must never under-count: CJK text and repeated short-token
/// runs sit near ~2 bytes/token, and bytes/4 would deny caching to contexts
/// genuinely above the floor. Erring toward caching costs at most the
/// one-time 25% cache-write premium; erring away forfeits the ~90% read
/// discount on every subsequent turn.
const BYTES_PER_TOKEN_ESTIMATE: usize = 2;

/// Inject `cache_control` markers at the 4 Anthropic prompt-cache zones
/// (moving-breakpoint layout, ported from openclaw/hermes-agent):
///
///   1. the last `system` block — stable prefix,
///   2. the last `tools` entry — stable prefix,
///   3. the previous user-boundary message (last user-role message before
///      the tail) — this is exactly the message the PREVIOUS turn marked as
///      its write point, so re-marking it guarantees the previous turn's
///      cached prefix is an addressable cache-read boundary this turn,
///      regardless of how many content blocks the new turn appended,
///   4. the last `messages` entry — the new cache-write point.
///
/// Zones 3+4 form a moving pair: turn *k* marks boundaries {k−1, k}, turn
/// *k+1* marks {k, k+1} — consecutive turns always overlap on one marked
/// boundary, keeping the layout cache-stable across turns.
///
/// Hard budget: at most [`ANTHROPIC_CACHE_CONTROL_LIMIT`] markers total,
/// counting any marker already present (the engine's
/// `mark_cache_boundaries` hint path may have pre-marked the tail block —
/// re-marking the same block is free). Message markers are spent
/// newest-first so the write point always wins over the read boundary.
///
/// Floor: when the estimated token size of the cacheable zones
/// (system + tools + messages) is below `min_prefix_tokens`, no markers are
/// injected — and any marker already present (the engine's hint path) is
/// STRIPPED, so a sub-floor request carries zero `cache_control` on the
/// wire. Caching a tiny context costs more (25% cache-write premium) than
/// it can save. Pass `0` to disable the floor. [`CacheTier::None`] strips
/// the same way: per-request "caching off" must mean off.
///
/// Idempotent — re-running on the same body produces the same output.
/// Skips zones cleanly when empty (no system text / no tools / no
/// messages). Markers are pure key inserts on existing objects: serde_json
/// maps are BTreeMap-backed (sorted keys), so injection never perturbs the
/// serialization order of any other field.
///
/// `tier` controls the ephemeral TTL marker:
///   - [`CacheTier::Ephemeral5m`] -> `{ "type": "ephemeral" }` (default 5m)
///   - [`CacheTier::Ephemeral1h`] -> `{ "type": "ephemeral", "ttl": "1h" }`
///   - [`CacheTier::None`]        -> strip + return (caching disabled for
///     this request — pre-existing hint markers must not leak to the wire)
pub fn apply_cache_zones(body: &mut Value, tier: CacheTier, min_prefix_tokens: usize) {
    let marker = match cache_control_marker(tier) {
        Some(m) => m,
        None => {
            strip_message_cache_markers(body);
            return;
        }
    };

    // Floor: for tiny contexts, skip injection AND remove any marker the
    // engine's hint path already placed — otherwise a sub-floor request
    // still pays cache-write behavior through the leaked tail marker.
    if estimated_prefix_tokens(body) < min_prefix_tokens {
        strip_message_cache_markers(body);
        return;
    }

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

    // Zones 3+4: the moving message-breakpoint pair, spent from whatever
    // budget the fixed zones (plus any pre-existing hint marker) left over.
    let fixed_markers = count_cache_markers(body.get("system"))
        + count_cache_markers(body.get("tools"))
        + count_message_cache_markers(body.get("messages"));
    let mut budget = ANTHROPIC_CACHE_CONTROL_LIMIT.saturating_sub(fixed_markers);

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut)
        && !messages.is_empty()
    {
        let last_idx = messages.len() - 1;

        // Zone 4 first (newest wins the budget): the tail write point.
        apply_message_zone_marker_budgeted(&mut messages[last_idx], &marker, &mut budget);

        // Zone 3: the previous user boundary — the last user-role message
        // strictly before the tail (= the previous turn's write point).
        if let Some(prev) = messages[..last_idx]
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        {
            apply_message_zone_marker_budgeted(&mut messages[prev], &marker, &mut budget);
        }
    }

    debug_assert!(
        count_cache_markers(body.get("system"))
            + count_cache_markers(body.get("tools"))
            + count_message_cache_markers(body.get("messages"))
            <= ANTHROPIC_CACHE_CONTROL_LIMIT,
        "cache_control markers must never exceed Anthropic's limit of {ANTHROPIC_CACHE_CONTROL_LIMIT}"
    );
}

/// Remove every `cache_control` marker from the messages zone — from
/// content blocks and (defensively) from message objects themselves. Used
/// when caching is off (config, per-request tier, or sub-floor context) to
/// undo the engine-hint marker that `build_messages` already translated.
/// Removal is byte-safe for siblings: serde_json objects are BTreeMap-backed
/// (sorted keys, no `preserve_order` swap-remove hazard), so deleting a key
/// cannot reorder the remaining fields.
fn strip_message_cache_markers(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if let Some(obj) = message.as_object_mut() {
            obj.remove("cache_control");
        }
        if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
            for block in blocks {
                if let Some(obj) = block.as_object_mut() {
                    obj.remove("cache_control");
                }
            }
        }
    }
}

/// Estimate the token size of the cacheable prompt zones (system + tools +
/// messages) as serialized-JSON bytes over [`BYTES_PER_TOKEN_ESTIMATE`].
fn estimated_prefix_tokens(body: &Value) -> usize {
    let mut bytes = 0usize;
    for zone in ["system", "tools", "messages"] {
        if let Some(v) = body.get(zone) {
            bytes += serde_json::to_string(v).map(|s| s.len()).unwrap_or(0);
        }
    }
    bytes / BYTES_PER_TOKEN_ESTIMATE
}

/// Count `cache_control` markers on the direct entries of a system/tools
/// zone array. `None` / non-array → 0.
fn count_cache_markers(zone: Option<&Value>) -> usize {
    zone.and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|e| e.get("cache_control").is_some())
                .count()
        })
        .unwrap_or(0)
}

/// Count `cache_control` markers anywhere in the messages zone: on content
/// blocks and (defensively) on message objects themselves.
fn count_message_cache_markers(zone: Option<&Value>) -> usize {
    zone.and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|m| {
                    let on_message = usize::from(m.get("cache_control").is_some());
                    let on_blocks = m
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter(|b| b.get("cache_control").is_some())
                                .count()
                        })
                        .unwrap_or(0);
                    on_message + on_blocks
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Mark `message` if (and only if) its target block is not already marked
/// and `budget` allows. An already-marked target is free (idempotent);
/// a fresh marker consumes one budget unit.
fn apply_message_zone_marker_budgeted(message: &mut Value, marker: &Value, budget: &mut usize) {
    // Already marked (e.g. by the engine's tail hint, or a repeat call)?
    // Nothing to spend.
    let already_marked = message
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.last())
        .map(|b| b.get("cache_control").is_some())
        .unwrap_or_else(|| message.get("cache_control").is_some());
    if already_marked {
        return;
    }
    if *budget == 0 {
        return;
    }
    apply_message_zone_marker(message, marker);
    *budget -= 1;
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
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        // Select the key ONCE and reuse it across the failover attempt: the same
        // credential is tried against both hosts (a region-locked key is valid on
        // exactly one), so re-selecting — which could rotate or hit a cooldown —
        // would defeat the retry.
        let key = self.select_key()?;
        let primary = self.effective_base_url();

        match self.try_stream(&primary, &key, &body).await {
            Ok(rx) => Ok(rx),
            // Region-locked-key failover: a credential rejected here (401/403)
            // may belong to the provider's alternate platform. When a fallback
            // host is configured and we haven't already pinned to it, retry the
            // SAME key there; pin it for the session on success so later requests
            // skip the primary's certain 401. See `compat.auth_fallback_base_url`.
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
                let rx = self.try_stream(&fallback, &key, &body).await?;
                self.pin_base_url(&fallback);
                Ok(rx)
            }
            Err(e) => Err(e),
        }
    }

    fn alias_key(&self) -> &str {
        &self.alias_key
    }

    /// Live model discovery via Anthropic's `GET /v1/models` endpoint. On any
    /// HTTP/parse failure we fall back to the static alias catalog — `/model`
    /// must never hard-fail.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
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

    /// `with_alias_key` overrides the offline `/model` fallback catalog so an
    /// Anthropic-wire reuse (e.g. MiniMax) surfaces its own models when live
    /// `GET /v1/models` discovery is unavailable — never Claude models.
    #[test]
    fn with_alias_key_overrides_offline_fallback_catalog() {
        let default = AnthropicProvider::new(
            "k",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        assert_eq!(default.alias_key(), "anthropic");

        let minimax = AnthropicProvider::new(
            "k",
            "https://api.minimax.io/anthropic",
            ProviderCompat::minimax_defaults(),
            DebugConfig::default(),
        )
        .with_alias_key("minimax");
        assert_eq!(minimax.alias_key(), "minimax");

        // The offline fallback now resolves MiniMax models, never Claude.
        let models = crate::alias_models(minimax.alias_key());
        assert!(!models.is_empty(), "minimax must list fallback models");
        assert!(
            models.iter().all(|m| m.id.starts_with("MiniMax-")),
            "every fallback model id must be a MiniMax model, got {:?}",
            models.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
    }

    /// A configured API key authenticates the request via the `x-api-key`
    /// header — Anthropic ships with API-key auth only.
    #[test]
    fn build_headers_sends_api_key_as_x_api_key() {
        let provider = AnthropicProvider::new(
            "explicit-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let key = provider.select_key().expect("single key selects");
        let headers = provider
            .build_headers(&key)
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

    /// With NO api key, `select_key` must fail with `MissingApiKey` rather
    /// than emit a blank credential producing an opaque 401.
    #[test]
    fn select_key_errors_when_no_api_key() {
        let provider = AnthropicProvider::new(
            "",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let result = provider.select_key();
        assert!(
            matches!(result, Err(ProviderError::MissingApiKey)),
            "no api key must surface MissingApiKey, got {result:?}"
        );
    }

    /// Multi-key rotation: after the current key is demoted via
    /// `mark_key_failure`, `select_key` rotates to a different key; once the
    /// new key succeeds it becomes sticky. A single configured key is the
    /// degenerate case and keeps returning that one key.
    #[test]
    fn multi_key_rotation_demotes_failing_key_then_succeeds() {
        let provider = AnthropicProvider::new(
            "key-a, key-b",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        // First selection picks one of the two keys.
        let first = provider.select_key().expect("a key is available");
        assert!(first == "key-a" || first == "key-b");

        // Simulate a 401/403/429 on that key — it must be demoted.
        provider.mark_key_failure(&first);
        let second = provider.select_key().expect("rotation finds the other key");
        assert_ne!(second, first, "failing key must rotate to the other key");

        // The second key succeeds — it becomes sticky for subsequent requests.
        provider.mark_key_success(&second);
        assert_eq!(
            provider.select_key().expect("sticky key"),
            second,
            "a succeeded key must stick as last-good"
        );
    }

    /// Single-key behavior is unchanged: `select_key` always returns the one
    /// configured key, even after a marked failure (it is the only option).
    #[test]
    fn single_key_behavior_unchanged() {
        let provider = AnthropicProvider::new(
            "solo-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        assert_eq!(provider.select_key().unwrap(), "solo-key");
        provider.mark_key_success("solo-key");
        assert_eq!(provider.select_key().unwrap(), "solo-key");
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
    fn all_four_zones_injected_when_present() {
        let mut body = body_with_all_zones();
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

        // System: last block has marker, first does not.
        let system = body["system"].as_array().unwrap();
        assert!(system[0].get("cache_control").is_none());
        assert_eq!(system[1]["cache_control"], marker_5m());

        // Tools: last tool has marker, first does not.
        let tools = body["tools"].as_array().unwrap();
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["cache_control"], marker_5m());

        // Messages: the moving pair — the tail (write point) AND the previous
        // user boundary both carry markers. Total = 4 (system+tools+2).
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[0]["content"][0]["cache_control"],
            marker_5m(),
            "previous user boundary must carry the read-boundary marker"
        );
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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);
        let after_first = body.clone();
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);
        assert_eq!(body, after_first, "second apply must be a no-op shape");
    }

    #[test]
    fn ttl_propagates_5m_vs_1h() {
        let mut body_5m = body_with_all_zones();
        apply_cache_zones(&mut body_5m, CacheTier::Ephemeral5m, 0);
        assert_eq!(body_5m["system"][1]["cache_control"], marker_5m());
        assert!(body_5m["system"][1]["cache_control"].get("ttl").is_none());

        let mut body_1h = body_with_all_zones();
        apply_cache_zones(&mut body_1h, CacheTier::Ephemeral1h, 0);
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
        apply_cache_zones(&mut body, CacheTier::None, 0);
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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

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
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);
        assert_eq!(body["messages"][0]["cache_control"], marker_5m());
    }

    // --- Task 9: moving breakpoint pair + budget + floor + gating -----------

    /// Collect the indices of messages carrying a cache_control marker
    /// (on any content block or on the message object itself).
    fn marked_message_indices(body: &Value) -> Vec<usize> {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.get("cache_control").is_some()
                    || m["content"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|b| b.get("cache_control").is_some())
                    })
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn total_markers(body: &Value) -> usize {
        serde_json::to_string(body)
            .unwrap()
            .matches("\"cache_control\"")
            .count()
    }

    fn user(text: &str) -> Value {
        json!({ "role": "user", "content": [ { "type": "text", "text": text } ] })
    }

    fn assistant(text: &str) -> Value {
        json!({ "role": "assistant", "content": [ { "type": "text", "text": text } ] })
    }

    /// The moving pair over a growing 3-turn conversation: every turn stays
    /// within Anthropic's 4-marker limit, marked positions move monotonically
    /// forward, and the previous turn's write point (its tail) is re-marked
    /// as this turn's read boundary — the cross-turn stability guarantee.
    #[test]
    fn three_turn_growing_conversation_markers_monotonic_and_bounded() {
        let turn_messages: [Vec<Value>; 3] = [
            vec![user("u1")],
            vec![user("u1"), assistant("a1"), user("u2")],
            vec![
                user("u1"),
                assistant("a1"),
                user("u2"),
                assistant("a2"),
                user("u3"),
            ],
        ];

        let mut prev_tail: Option<usize> = None;
        let mut prev_max: usize = 0;
        for (turn, messages) in turn_messages.into_iter().enumerate() {
            let mut body = json!({
                "system": [ { "type": "text", "text": "sys" } ],
                "tools": [ { "name": "t", "description": "d" } ],
                "messages": messages
            });
            apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

            assert!(
                total_markers(&body) <= 4,
                "turn {turn}: markers must never exceed Anthropic's limit of 4"
            );

            let marked = marked_message_indices(&body);
            let tail = body["messages"].as_array().unwrap().len() - 1;
            assert!(
                marked.contains(&tail),
                "turn {turn}: the tail write point must always be marked"
            );
            assert!(
                marked.iter().all(|&i| i >= prev_max.min(tail)),
                "turn {turn}: marked positions must move monotonically forward, got {marked:?}"
            );
            if let Some(prev) = prev_tail {
                assert!(
                    marked.contains(&prev),
                    "turn {turn}: the previous turn's write point (index {prev}) must be \
                     re-marked as this turn's read boundary, got {marked:?}"
                );
            }
            prev_max = *marked.iter().max().unwrap();
            prev_tail = Some(tail);
        }
    }

    /// Budget enforcement with a pre-existing hint marker: the engine's
    /// `mark_cache_boundaries` path may have already marked the tail block
    /// via `build_messages`. Re-marking it is free; the total must still
    /// come out at exactly the 4-marker limit, never above.
    #[test]
    fn budget_holds_at_four_with_preexisting_hint_marker() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "sys" } ],
            "tools": [ { "name": "t", "description": "d" } ],
            "messages": [
                user("u1"),
                assistant("a1"),
                // Tail pre-marked, as the hint path emits it.
                { "role": "user", "content": [
                    { "type": "text", "text": "u2",
                      "cache_control": { "type": "ephemeral" } }
                ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

        assert_eq!(
            total_markers(&body),
            4,
            "system + tools + boundary pair must land exactly at the limit"
        );
        assert_eq!(
            marked_message_indices(&body),
            vec![0, 2],
            "previous user boundary (0) and tail (2) must be the marked pair"
        );
    }

    /// When the fixed zones leave only one message slot, the tail write
    /// point wins it and the read boundary is skipped — never a 5th marker.
    #[test]
    fn tail_wins_budget_when_only_one_message_slot_remains() {
        // Pre-marked NON-tail message eats one budget unit (pathological
        // upstream state), leaving one slot for two candidates.
        let mut body = json!({
            "system": [ { "type": "text", "text": "sys" } ],
            "tools": [ { "name": "t", "description": "d" } ],
            "messages": [
                { "role": "user", "content": [
                    { "type": "text", "text": "u1",
                      "cache_control": { "type": "ephemeral" } }
                ] },
                assistant("a1"),
                user("u2"),
                assistant("a2"),
                user("u3")
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

        assert_eq!(total_markers(&body), 4, "hard cap must hold");
        let marked = marked_message_indices(&body);
        assert!(
            marked.contains(&4),
            "the tail write point must win the last budget slot, got {marked:?}"
        );
        assert!(
            !marked.contains(&2),
            "the read boundary must be skipped when the budget is spent, got {marked:?}"
        );
    }

    /// Gap-1+gap-2 coupling: the engine sends TWO hints — the permanent
    /// compaction anchor plus the tail — and the budget lands at exactly 4
    /// (system + tools + anchor + tail). The moving previous-boundary marker
    /// YIELDS its slot to the anchor: the anchor guarantees the long prefix.
    #[test]
    fn anchor_plus_tail_hints_budget_exactly_four_prev_boundary_yields() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "sys" } ],
            "tools": [ { "name": "t", "description": "d" } ],
            "messages": [
                // Permanent anchor (stubbed compaction message), hint-marked
                // by mark_cache_boundaries via build_messages.
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "w1", "name": "Write",
                      "input": { "_args_cleared": "[stub]" },
                      "cache_control": { "type": "ephemeral" } }
                ] },
                user("r1"),
                assistant("a2"),
                user("u2"),
                assistant("a3"),
                // Tail, hint-marked by mark_cache_boundaries.
                { "role": "user", "content": [
                    { "type": "text", "text": "u3",
                      "cache_control": { "type": "ephemeral" } }
                ] }
            ]
        });
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, 0);

        assert_eq!(
            total_markers(&body),
            4,
            "system + tools + anchor + tail must land exactly at the limit"
        );
        let marked = marked_message_indices(&body);
        assert_eq!(
            marked,
            vec![0, 5],
            "anchor (0) and tail (5) hold the message slots; the moving \
             previous-boundary marker (3) must yield, got {marked:?}"
        );
    }

    /// Config-off stripping (Codex finding 1) also removes the anchor
    /// marker: with caching disabled, neither the tail hint nor the anchor
    /// hint may leak to the wire.
    #[test]
    fn config_off_strips_anchor_and_tail_hints() {
        let off = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false);

        let mut req = cache_req(None);
        req.messages = vec![
            {
                let mut m = wcore_types::message::Message::new(
                    wcore_types::message::Role::Assistant,
                    vec![wcore_types::message::ContentBlock::Text {
                        text: "anchor".into(),
                    }],
                );
                m.cache_breakpoint = Some(wcore_types::message::MessageCacheHint::Breakpoint);
                m
            },
            wcore_types::message::Message::new(
                wcore_types::message::Role::User,
                vec![wcore_types::message::ContentBlock::Text { text: "mid".into() }],
            ),
            {
                let mut m = wcore_types::message::Message::new(
                    wcore_types::message::Role::User,
                    vec![wcore_types::message::ContentBlock::Text {
                        text: "tail".into(),
                    }],
                );
                m.cache_breakpoint = Some(wcore_types::message::MessageCacheHint::Breakpoint);
                m
            },
        ];
        let body = off.build_request_body(&req);
        assert_eq!(
            total_markers(&body),
            0,
            "config-off must strip BOTH the anchor and tail hint markers"
        );
    }

    // --- Task 9: min_prefix_tokens floor ------------------------------------

    /// Tiny context + the default 1024-token floor → no markers at all.
    /// Cache-writing a tiny prefix costs a 25% premium for nothing.
    #[test]
    fn tiny_context_default_floor_skips_all_markers() {
        let default_floor_provider = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let body = default_floor_provider.build_request_body(&cache_req(None));
        assert_eq!(
            total_markers(&body),
            0,
            "a tiny prompt under the 1024-token floor must get no cache_control markers"
        );
    }

    /// A prompt that clears the floor gets the full marker layout with the
    /// same default-floor provider.
    #[test]
    fn large_context_clears_default_floor_and_gets_markers() {
        let default_floor_provider = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let mut req = cache_req(None);
        // ~8k bytes ≈ ~2k estimated tokens — comfortably over the 1024 floor.
        req.system = "x".repeat(8_192);
        let body = default_floor_provider.build_request_body(&req);
        assert!(
            total_markers(&body) >= 2,
            "a prompt over the floor must carry cache_control markers"
        );
        assert_eq!(body["system"][0]["cache_control"], marker_5m());
    }

    /// The floor gates on the estimate, exactly at the boundary semantics
    /// of `estimated < floor` → skip.
    #[test]
    fn floor_boundary_semantics() {
        let mut body = json!({
            "system": [ { "type": "text", "text": "sys" } ],
            "messages": [ user("hi") ]
        });
        let estimate = super::estimated_prefix_tokens(&body);
        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, estimate + 1);
        assert_eq!(total_markers(&body), 0, "estimate < floor must skip");

        apply_cache_zones(&mut body, CacheTier::Ephemeral5m, estimate);
        assert!(total_markers(&body) > 0, "estimate >= floor must inject");
    }

    // --- Task 9: config-off byte identity + family gating -------------------

    /// A request carrying the engine's tail cache hint, exactly as
    /// `wcore-observability::cache::mark_cache_boundaries` stamps it before
    /// every dispatch. The production path ALWAYS carries this for
    /// anthropic-family compat — a caching test without it misses the
    /// hint-translation leak in `build_messages` (Codex audit finding 1).
    fn hinted_req(tier: Option<CacheTier>) -> LlmRequest {
        let mut req = cache_req(tier);
        req.messages
            .last_mut()
            .expect("cache_req always has a message")
            .cache_breakpoint = Some(wcore_types::message::MessageCacheHint::Breakpoint);
        req
    }

    /// `prompt_caching = false` (config off): the request body is
    /// byte-identical to a body that never saw the cache injector — EVEN
    /// when the engine's tail hint is present (which `build_messages`
    /// translates into a wire marker before the config gate runs).
    #[test]
    fn config_off_body_is_byte_identical_to_uninjected() {
        let off = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false);

        let req = hinted_req(Some(CacheTier::Ephemeral1h));
        let body = off.build_request_body(&req);
        let bytes = serde_json::to_string(&body).unwrap();
        assert!(
            !bytes.contains("cache_control"),
            "config-off body must carry no cache_control anywhere"
        );

        let expected = json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "system": [ { "type": "text", "text": "you are a test" } ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "hi" } ] }
            ],
            "stream": true
        });
        assert_eq!(
            bytes,
            serde_json::to_string(&expected).unwrap(),
            "config-off body must serialize byte-identically to the uninjected shape"
        );
    }

    /// Sub-floor request WITH the engine hint: the leaked tail marker from
    /// `build_messages` must be stripped, not merely left uninjected —
    /// a tiny request must carry zero `cache_control` on the wire.
    #[test]
    fn sub_floor_request_with_engine_hint_emits_zero_markers() {
        let default_floor_provider = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let body = default_floor_provider.build_request_body(&hinted_req(None));
        assert_eq!(
            total_markers(&body),
            0,
            "a sub-floor request must strip the engine's hint marker, not leak it"
        );
    }

    /// `cache_tier = Some(None)` (per-request caching off) strips the
    /// engine-hint marker the same way the config gate does.
    #[test]
    fn cache_tier_none_strips_engine_hint_marker() {
        let body = provider().build_request_body(&hinted_req(Some(CacheTier::None)));
        assert_eq!(
            total_markers(&body),
            0,
            "per-request caching-off must strip the hint marker from the wire body"
        );
    }

    /// Marker stripping is byte-safe for siblings: stripping a hinted body
    /// reproduces the never-hinted body exactly (BTreeMap removal cannot
    /// reorder remaining keys).
    #[test]
    fn strip_reproduces_unhinted_body_bytes() {
        let off = AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false);
        let hinted = off.build_request_body(&hinted_req(None));
        let unhinted = off.build_request_body(&cache_req(None));
        assert_eq!(
            serde_json::to_string(&hinted).unwrap(),
            serde_json::to_string(&unhinted).unwrap(),
            "hinted and never-hinted config-off bodies must be byte-identical"
        );
    }

    /// Family gating: the message-hint translation in `build_messages` obeys
    /// `compat.cache_message_breakpoints()` — Anthropic-family compat emits
    /// the marker, OpenAI-family compat leaves the message untouched.
    #[test]
    fn family_gating_hint_translation_anthropic_yes_openai_no() {
        let mut msg = wcore_types::message::Message::new(
            wcore_types::message::Role::User,
            vec![wcore_types::message::ContentBlock::Text { text: "hi".into() }],
        );
        msg.cache_breakpoint = Some(wcore_types::message::MessageCacheHint::Breakpoint);
        let messages = vec![msg];

        let anthropic_wire =
            anthropic_shared::build_messages(&messages, &ProviderCompat::anthropic_defaults());
        assert!(
            anthropic_wire[0]["content"][0]
                .get("cache_control")
                .is_some(),
            "anthropic-family compat must translate the hint into cache_control"
        );

        let openai_wire =
            anthropic_shared::build_messages(&messages, &ProviderCompat::openai_defaults());
        assert!(
            !serde_json::to_string(&openai_wire)
                .unwrap()
                .contains("cache_control"),
            "openai-family compat must never emit cache_control"
        );
    }

    /// Family gating at the provider level: an Anthropic-wire third party
    /// with unverified caching support (the MiniMax reuse) is constructed
    /// `with_cache(false)` — its bodies carry no markers.
    #[test]
    fn family_gating_minimax_reuse_emits_no_markers() {
        let minimax = AnthropicProvider::new(
            "k",
            "https://api.minimax.io/anthropic",
            ProviderCompat::minimax_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false)
        .with_alias_key("minimax");
        let body = minimax.build_request_body(&cache_req(Some(CacheTier::Ephemeral5m)));
        assert_eq!(
            total_markers(&body),
            0,
            "a cache-disabled Anthropic-wire reuse must emit no cache_control"
        );
    }

    /// Byte-stability: injecting markers must not perturb the serialization
    /// of anything else — stripping the injected `cache_control` keys from
    /// the marked body must reproduce the unmarked body byte-for-byte.
    #[test]
    fn injection_does_not_perturb_sibling_serialization() {
        let mut marked = body_with_all_zones();
        apply_cache_zones(&mut marked, CacheTier::Ephemeral5m, 0);

        fn strip(v: &mut Value) {
            match v {
                Value::Object(map) => {
                    map.remove("cache_control");
                    for child in map.values_mut() {
                        strip(child);
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        strip(child);
                    }
                }
                _ => {}
            }
        }
        strip(&mut marked);

        assert_eq!(
            serde_json::to_string(&marked).unwrap(),
            serde_json::to_string(&body_with_all_zones()).unwrap(),
            "removing only the injected markers must reproduce the original bytes"
        );
    }

    // --- W8 v0.6.3: build_request_body consumes request.cache_tier ---------

    /// Test provider with the breakpoint floor disabled — the tiny request
    /// bodies these tests build would otherwise (correctly) fall under the
    /// default 1024-token floor and get no markers at all. The floor itself
    /// is covered by the dedicated floor tests below.
    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key",
            "https://api.anthropic.com",
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_min_prefix_tokens(0)
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
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
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

    // --- Output-side opt (Part A): stop_sequences -> body["stop_sequences"] ---

    #[test]
    fn build_request_body_omits_stop_sequences_when_empty() {
        let body = provider().build_request_body(&cache_req(None));
        assert!(
            body.get("stop_sequences").is_none(),
            "empty stop_sequences must emit no stop field (back-compat)"
        );
    }

    #[test]
    fn build_request_body_emits_stop_sequences_when_present() {
        let mut req = cache_req(None);
        req.stop_sequences = vec!["\n\nLet me know if".into(), "\n\nFeel free to".into()];
        let body = provider().build_request_body(&req);
        assert_eq!(
            body["stop_sequences"],
            json!(["\n\nLet me know if", "\n\nFeel free to"]),
            "Anthropic must emit stops under the `stop_sequences` key"
        );
    }

    /// #112: the Anthropic Messages API MANDATES `max_tokens` — even when a
    /// caller (wrongly) flags `omit_max_tokens`, the body must still carry the
    /// sized value. The engine never sets the flag for anthropic (the compat
    /// preset is off), but this pins the provider-side invariant regardless.
    #[test]
    fn build_request_body_always_sends_max_tokens_even_when_omit_flagged() {
        let mut req = cache_req(None);
        req.max_tokens = 8_192;
        req.omit_max_tokens = true;
        let body = provider().build_request_body(&req);
        assert_eq!(
            body["max_tokens"],
            json!(8_192),
            "anthropic must ALWAYS serialize max_tokens (Messages API mandate)"
        );
    }
}
