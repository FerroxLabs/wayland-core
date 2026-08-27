// Configuration-driven provider compatibility layer.
// Each provider type has default presets; users can override any field via config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Concrete model ids to substitute for each smart-routing tier.
///
/// Finding #174: the engine classifies every turn into a `RoutingTier`
/// (`cheap` / `balanced` / `premium`) and stamps `LlmRequest::routing_hint`,
/// but nothing acted on it — every turn ran on the user's premium model. This
/// map is the opt-in that lets a `cheap` (or `balanced`) hint actually swap to
/// a smaller, cheaper model before dispatch.
///
/// Each field is `Option<String>` so the user can configure only the tiers they
/// care about (e.g. just `cheap`). A tier left `None` means "no swap for this
/// tier — keep the originally requested model". The presence of a configured
/// entry for the hinted tier is itself the enable switch: with the whole map
/// absent (the default), behaviour is unchanged for every existing user.
///
/// `premium` is accepted for completeness/symmetry but the engine never
/// downgrades a `premium` hint, so configuring it has no effect on routing; it
/// exists so a user can document the full mapping in one place.
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct TierModels {
    /// Model id used when the router classifies the turn as `cheap`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheap: Option<String>,
    /// Model id used when the router classifies the turn as `balanced`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balanced: Option<String>,
    /// Model id for the `premium` tier. Documentation-only: the engine never
    /// downgrades a premium hint, so this is not consulted for routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premium: Option<String>,
}

impl TierModels {
    /// Resolve the configured model id for a routing-tier label, if any.
    ///
    /// `tier` is the lowercase tier name as produced by
    /// `RoutingTier`'s serde rename / `RoutingDecision::to_hint` (`"cheap"`,
    /// `"balanced"`, `"premium"`). Returns `None` for the premium tier and for
    /// any tier that has no configured model — both mean "no swap".
    pub fn model_for_tier(&self, tier: &str) -> Option<&str> {
        match tier {
            "cheap" => self.cheap.as_deref(),
            "balanced" => self.balanced.as_deref(),
            // Premium is intentionally never returned for routing: the engine
            // must not downgrade (or "swap") a premium turn.
            _ => None,
        }
    }
}

/// Where each of the four `cost_per_*_token` rates in a resolved
/// [`ProviderCompat`] came from.
///
/// `true` on an axis means the USER stated that rate (config file or the
/// `/config` Expert tier), so it is an authoritative statement about the
/// endpoint actually being called. `false` — the default, and the value
/// after any deserialization — means the rate was inherited from a built-in
/// per-PROVIDER preset.
///
/// A preset rate is the vendor's coarse list price (`anthropic_defaults()`
/// carries the Opus row for EVERY Anthropic model). Applying it to an
/// arbitrary model produces a number that is useful as a conservative
/// admission ceiling and false as a report of spend, so the two uses are
/// kept apart: the engine charges the budget with it and refuses to print
/// it. Measured before this split: `anthropic/claude-3-5-sonnet-20241022`
/// was reported to the user as `$90.00` where the real price is `$18.00`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CostRateProvenance {
    pub input: bool,
    pub output: bool,
    pub cache_read: bool,
    pub cache_write: bool,
}

/// Provider-level compatibility settings.
/// Each field is Option — None means "use provider-type default".
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderCompat {
    /// Field name for max tokens in request body.
    /// Default: "max_tokens" for all providers.
    pub max_tokens_field: Option<String>,

    /// Between-bytes timeout for streaming provider responses, in milliseconds.
    /// `None` uses the hardened global default. This is a generic transport
    /// policy override for endpoints whose expected silent gaps differ from
    /// that default; it must never be selected by inspecting a provider URL.
    pub read_timeout_ms: Option<std::num::NonZeroU64>,

    /// Merge consecutive assistant messages (text concat + tool_calls merge).
    /// Default: true for openai.
    pub merge_assistant_messages: Option<bool>,

    /// Remove tool_use blocks that have no corresponding tool_result.
    /// Default: true for openai.
    pub clean_orphan_tool_calls: Option<bool>,

    /// Deduplicate tool results with same tool_call_id (keep last).
    /// Default: true for openai.
    pub dedup_tool_results: Option<bool>,

    /// Ensure messages alternate user/assistant (insert filler if needed).
    /// Default: true for anthropic/bedrock/vertex.
    pub ensure_alternation: Option<bool>,

    /// Merge consecutive same-role messages into one.
    /// Default: true for anthropic/bedrock/vertex.
    pub merge_same_role: Option<bool>,

    /// Sanitize JSON schemas for strict providers (remove additionalProperties, etc.).
    /// Default: true for bedrock.
    pub sanitize_schema: Option<bool>,

    /// Text patterns to strip from message history before sending.
    /// Default: empty.
    pub strip_patterns: Option<Vec<String>>,

    /// Auto-generate tool IDs when missing.
    /// Default: true for anthropic/bedrock/vertex.
    pub auto_tool_id: Option<bool>,

    /// Custom API path appended to base_url for chat completions.
    /// Default: "/v1/chat/completions" for OpenAI provider.
    /// Override to "/chat/completions" for providers like Gemini that include
    /// version prefix in the base URL itself.
    pub api_path: Option<String>,

    /// Whether this provider supports extended thinking (Anthropic-style).
    /// Default: true for anthropic/bedrock/vertex, false for openai.
    pub supports_thinking: Option<bool>,

    /// Whether this provider supports reasoning_effort (OpenAI-style).
    /// Default: false for anthropic/bedrock/vertex, true for openai.
    pub supports_effort: Option<bool>,

    /// Available effort levels for this provider (e.g., ["low", "medium", "high"]).
    /// Only meaningful when supports_effort is true.
    pub effort_levels: Option<Vec<String>>,

    /// Whether this provider honours explicit `cache_control` breakpoint
    /// markers on individual messages (in addition to the system prompt and
    /// tools list). Anthropic-family providers (anthropic, bedrock, vertex)
    /// honour up to four per request; OpenAI and Gemini do not.
    ///
    /// When `Some(true)`, the `wcore-observability::cache::mark_cache_boundaries`
    /// helper places one additional breakpoint at the tail of the prompt to
    /// raise multi-turn cache hit rate; `Some(false)` / `None` disables the
    /// extra marker for this provider.
    pub cache_message_breakpoints: Option<bool>,

    /// Whether this endpoint is expected to SERVE prompt-cached input, i.e.
    /// whether a warm session that reads zero cached tokens is a defect
    /// rather than the endpoint simply having no cache.
    ///
    /// This is a capability statement, not a switch: nothing in the request
    /// changes. It is read only by the engine's warm-session cache-health
    /// probe, which without it cannot tell "this provider has no prompt
    /// cache" from "this provider has one and we are getting nothing from
    /// it" - the two are identical in the response (all-zero cache counters
    /// forever). Issue #559 was the second case and went unreported for
    /// 77.7M input tokens because of that ambiguity.
    ///
    /// `None` resolves to `false`: an endpoint we know nothing about is never
    /// accused of a broken cache. Set `Some(true)` only where the capability
    /// is established - see the presets.
    ///
    /// Because `Some(true)` is an accusation, it must never arrive by
    /// accident: any preset built with `..Self::some_other_defaults()` struct
    /// update inherits this field unless it clears it explicitly. The pinning
    /// test `only_justified_presets_expect_a_served_prompt_cache` reads the
    /// RESOLVED value off every real preset and fails if an unjustified one
    /// turns true.
    pub prompt_cache_expected: Option<bool>,

    /// W6 — structured provider identity for trace and cost attribution.
    /// Replaces the W1 `supports_thinking()` heuristic in `wcore-agent`.
    /// Set to one of: "anthropic" | "bedrock" | "vertex" | "openai" | "ollama".
    /// Defaults to "unknown" when missing.
    pub provider_type: Option<String>,

    /// W6 F7 — USD per input token. Multiply by token count for per-turn cost.
    /// Set in each provider preset; `None` means no compatibility fallback rate.
    /// Per-provider list price (NOT per-model); per-model pricing is W6.1.
    pub cost_per_input_token: Option<f64>,

    /// W6 F7 — USD per output token.
    pub cost_per_output_token: Option<f64>,

    /// W6 F7 — USD per cached input token read.
    pub cost_per_cache_read_token: Option<f64>,

    /// W6 F7 — USD per cached input token written (cache creation).
    pub cost_per_cache_write_token: Option<f64>,

    /// Declare an all-zero compatibility rate as known-free rather than
    /// unpriced. Catalog pricing still wins when an exact model row exists.
    /// Leave unset for zero-valued sentinels whose real price is unknown.
    pub cost_is_known_free: Option<bool>,

    /// Provenance of the four `cost_per_*_token` rates above — see
    /// [`CostRateProvenance`].
    ///
    /// Derived positionally by [`ProviderCompat::merge`], whose second
    /// argument is by contract the user's overlay. Deliberately
    /// `#[serde(skip)]`: a config file must not be able to ASSERT authority
    /// (it earns authority by carrying the rate), and a serialization
    /// round-trip must degrade to the fail-safe "not authoritative" rather
    /// than to "trust the preset".
    #[serde(skip)]
    pub cost_rate_provenance: CostRateProvenance,

    /// Whether the destination endpoint optimizes request *input* server-side.
    ///
    /// - `Some("router")` — the endpoint is a routing layer (e.g. a Flux- or
    ///   OpenRouter-class server-side router) that performs its own input
    ///   optimization before forwarding to the upstream model. When set, the
    ///   engine should *defer* client-side token-optimization passes to avoid
    ///   doing redundant (and potentially conflicting) work.
    /// - `Some("client")` / `None` — the endpoint is a direct provider that
    ///   expects the client to optimize input itself; client-side passes run.
    ///
    /// This is a vendor-neutral *capability* flag — it records only what the
    /// endpoint does, not any product-specific behaviour. No billing, savings,
    /// or arbitrage logic lives here.
    pub input_optimization: Option<String>,

    /// Token-opt: when `true` (the default), the engine compacts verbose Bash
    /// output (cargo/git/test/grep) before it enters the model's transcript.
    /// `None` ⇒ use the resolver default (ON). See `compact_bash()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_bash: Option<bool>,

    /// Whether to send `stream_options: {include_usage: true}` on OpenAI-format
    /// streaming requests. `None`/`Some(true)` (default) sends it so the engine
    /// receives token-usage accounting in the final stream chunk. Some generic
    /// self-hosted OpenAI-compatible servers (older vLLM, llama.cpp, some Qwen
    /// deployments) reject the unknown `stream_options` field with HTTP 400 —
    /// set `Some(false)` (`[compat] include_usage_in_stream = false`) for those
    /// endpoints to drop the field at the cost of in-stream usage stats.
    /// See FerroxLabs/wayland#86.
    pub include_usage_in_stream: Option<bool>,

    /// Force the OpenAI chat-vs-responses API surface for this provider,
    /// overriding the per-model family default
    /// (`openai_compat::model_uses_responses_api`).
    ///
    /// - `Some(true)` — always use the Responses API (`POST /v1/responses`),
    ///   e.g. a custom endpoint that requires it for an unrecognized model id.
    /// - `Some(false)` — always use Chat Completions (`POST /v1/chat/completions`),
    ///   e.g. an openai-compat gateway that proxies `gpt-5*` over the chat
    ///   surface.
    /// - `None` (default) — defer to the model-family predicate: the `gpt-5*`
    ///   family routes to Responses, everything else to Chat Completions.
    ///
    /// The `gpt-5*` family is rejected at `/v1/chat/completions` upstream, so
    /// the default `None` already does the right thing for native OpenAI.
    pub uses_responses_api: Option<bool>,

    /// F27: Force whether the request body uses `max_completion_tokens` instead
    /// of `max_tokens` for this provider, overriding the per-model family
    /// default (`openai_compat::wants_max_completion_tokens`'s prefix heuristic).
    ///
    /// - `Some(true)` — always send `max_completion_tokens` (e.g. a gateway that
    ///   serves only reasoning-family models behind a custom id).
    /// - `Some(false)` — always send `max_tokens` (e.g. an openai-compat backend
    ///   that doesn't understand `max_completion_tokens`).
    /// - `None` (default) — defer to the model-family prefix heuristic: the
    ///   `o1*`/`o3*` reasoning families and the `gpt-5*` family use
    ///   `max_completion_tokens`, everything else uses `max_tokens`.
    pub uses_max_completion_tokens: Option<bool>,

    /// Azure OpenAI authentication mode (R77). Only consulted for the
    /// `AzureOpenAI` provider at bootstrap. `None`/`api-key` sends the Azure
    /// `api-key` header from the configured key; `aad-bearer` switches to an
    /// Entra-ID / OAuth bearer token sourced from the `AZURE_AD_TOKEN`
    /// environment variable (the crate owns no token acquisition/refresh).
    /// Set via `[compat] azure_auth_mode = "aad-bearer"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_auth_mode: Option<crate::config::AzureAuthMode>,

    /// Alternate API base URL to retry against when the primary `base_url`
    /// rejects the credential with a 401. Some providers run two region-locked
    /// platforms that share the wire protocol but NOT the key namespace, so a
    /// valid key issued on one platform 401s on the other's host. MiniMax is the
    /// motivating case (`api.minimax.io` vs `api.minimaxi.com` — a key works on
    /// exactly one, verified live 2026-06-18). When set, a 401 on the primary
    /// transparently retries the same key against this host and pins whichever
    /// authenticates for the rest of the session, so the user never has to know
    /// which region issued their key. `None` (the default) disables failover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_fallback_base_url: Option<String>,

    /// Whether the provider accepts the OpenAI `stop` parameter. The engine
    /// attaches "fluff" stop sequences as an output token-optimization on
    /// client-optimized routes; some providers' reasoning models reject the
    /// `stop` parameter outright with a 400 (xAI's `grok-4.3`: *"Model grok-4.3
    /// does not support parameter stop"*, verified live 2026-06-18). Set
    /// `false` to suppress the optimization so those models work. `None`
    /// defaults to `true` — every existing provider keeps sending `stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_stop_param: Option<bool>,

    /// Whether the provider's chat-completions API *requires* every historical
    /// assistant message to carry `reasoning_content` once any turn produced
    /// thinking. The strict reasoner endpoints (DeepSeek Reasoner, Moonshot/Kimi)
    /// 400 the request otherwise, so for them we must replay prior-turn thinking.
    ///
    /// `None`/`Some(false)` (the default) DROPS historical thinking blocks at the
    /// wire — they are billed as fresh input every turn but the model does not
    /// need them, so re-sending them is pure recurring cost (finding #174). This
    /// matches the Anthropic/Bedrock/Vertex adapters, which already drop
    /// historical thinking. Only set `Some(true)` for providers whose API
    /// rejects the request without the replay.
    ///
    /// Note: this governs the Chat Completions path only. The Responses API path
    /// (`openai_responses.rs`) drops ALL reasoning items unconditionally, because
    /// there reasoning items are protocol-linked to encrypted ids we do not
    /// persist and re-sending them triggers validation errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replays_thinking_in_history: Option<bool>,

    /// Whether to re-serialize the internal `extra_content` blob (captured from
    /// an inbound `tool_calls[].extra_content`, e.g. Gemini's
    /// `extra_content.google.thought` routing marker) back onto OUTBOUND
    /// `tool_calls` on the Chat Completions path.
    ///
    /// Defaults to `false`: `extra_content` is an internal-only field and must
    /// NOT be echoed to providers that reject unknown fields. On long-context
    /// replay, strict OpenAI-compat endpoints (e.g. Fireworks / GLM-5 via the
    /// Flux router) 400 with "Extra inputs are not permitted, field:
    /// messages[N].tool_calls[0].extra_content" (wayland-core#120). Only the
    /// Google/Gemini preset sets `Some(true)`, since that endpoint emitted the
    /// field and tolerates its round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit_tool_call_extra_content: Option<bool>,

    /// Finding #174: per-tier model substitution for smart routing.
    ///
    /// The engine classifies every turn into a `RoutingTier` and stamps a
    /// `routing_hint`; when this map is set and configures a model for the
    /// hinted tier, the engine rewrites `LlmRequest::model` to that tier model
    /// before dispatch (cheap/balanced only, never premium, never for
    /// image/vision turns). Token/cost accounting is attributed to the swapped
    /// model.
    ///
    /// `None` (the default) means no swap ever happens — behaviour is unchanged
    /// for every user who has not opted in. Configure via:
    /// `[compat.tier_models] cheap = "claude-haiku-4" balanced = "..."`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_models: Option<TierModels>,

    /// #344/#359 — the provider's hard cap on the number of tools per request
    /// (OpenAI's tool-array limit is 128). `None` = no cap. Enforced engine-side
    /// after MCP curation, since MCP servers can push the array past the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tools: Option<usize>,

    /// Crucible #3: whether this provider accepts an explicit `temperature`
    /// body field. `None`/`Some(true)` (the default) emits the request's
    /// temperature when one is set; `Some(false)` suppresses it for endpoints
    /// that reject the parameter. This is a coarse PROVIDER-level switch; the
    /// per-MODEL exclusion of the OpenAI `o1*`/`o3*` reasoning families (which
    /// fix temperature at 1.0) is handled separately by
    /// `openai_compat::accepts_temperature(model)`. Following the no-hardcoded-
    /// quirks rule, temperature emission is gated by this flag + that model
    /// predicate, never by `base_url.contains(...)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature: Option<bool>,

    /// #112 — whether this provider tolerates OMITTING the max-tokens wire
    /// field entirely, letting the served model's natural output ceiling
    /// apply. Consulted by the engine only when the model is unknown to the
    /// `wcore_config::limits` registry AND the user omitted `--max-tokens`
    /// (no CLI flag, no non-default TOML value); a known model or an explicit
    /// user cap always sends a sized value.
    ///
    /// `Some(true)` for the gemini / openrouter / flux-router presets (their
    /// endpoints default the field per served model). `None`/`Some(false)`
    /// (the default) keeps sending a sized value — REQUIRED for anthropic
    /// (the Messages API mandates `max_tokens`) and the safe choice for
    /// generic self-hosted openai-compatible endpoints (vLLM et al. may 400
    /// without the field or default to a tiny ceiling like 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omit_max_tokens_when_unsized: Option<bool>,

    /// #863 F2 — whether this ENDPOINT speaks the Flux loop-ownership
    /// anti-collision handshake (`X-Flux-Loop-Owner` / `metadata.loop_owner`,
    /// `X-Flux-Verify` / `metadata.flux_verify`, and the `x-flux-loop-engaged`
    /// response echo).
    ///
    /// `Some(true)` for `flux_router_defaults` only; every other provider
    /// leaves it `None` (→ false) and no loop-provenance field ever reaches
    /// their wire. This is the single, vendor-neutral place that decides it —
    /// deliberately NOT a `base_url.contains("fluxrouter")` test and
    /// deliberately NOT `is_flux_tier_alias(&request.model)`:
    ///
    /// - a URL test is the hardcoded-provider-quirk shape this whole field
    ///   family exists to remove, and
    /// - a model-name test would be WRONG for this contract. The #282 `x-wl-*`
    ///   headers gate on the tier alias because they are routing hints that
    ///   only mean something to the router's own tiers. Loop ownership is not a
    ///   routing hint: Flux honours `loop_owner` "regardless of alias" (F2), so
    ///   an alias gate would silently drop the marking on a driver turn pinned
    ///   to a concrete model id — the precise case where a collision is
    ///   undetectable from either side.
    ///
    /// A self-hosted or Anthropic-wire deployment that also speaks the
    /// handshake turns it on here rather than by being pattern-matched.
    pub flux_loop_provenance: Option<bool>,

    /// #648 — whether the provider's served model(s) accept inline image
    /// (vision) input. Consulted by `OpenAIProvider::build_messages`: when
    /// `false` (or unset) a `ContentBlock::Image` is NOT emitted as an OpenAI
    /// `image_url` multipart part — text-only endpoints 400 on it — and instead
    /// the shared `[image omitted: model not vision-capable]` text placeholder
    /// is appended, matching cohere / bedrock (mistral) / wayland-ollama.
    ///
    /// `None`/`Some(false)` (the default) is the SAFE choice: omit the image
    /// (soft degradation) rather than risk a hard 400 on a non-vision model.
    /// Presets set `Some(true)` only for providers whose catalog hosts
    /// vision-capable models (openai, azure-openai, openrouter, together,
    /// fireworks, nvidia, xai, qwen, groq, flux-router, mistral).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,

    /// Whether this provider/model contract accepts tool definitions and can
    /// return tool calls. `None` is unknown and therefore fail-closed for a
    /// fallback carrying tools; provider presets set this only where the wire
    /// path is implemented and exercised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tools: Option<bool>,

    /// Whether this provider/model contract supports caller-constrained
    /// structured output (for example JSON Schema response formats). Unknown
    /// support is fail-closed whenever a request explicitly requires it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_structured_output: Option<bool>,

    /// F-27C3-04 — the model id to send to this provider's OpenAI-wire
    /// `/images/generations` endpoint.
    ///
    /// The image endpoint's model namespace is **per provider** and does not
    /// follow the chat model id: native OpenAI serves `gpt-image-1`, our
    /// FluxRouter serves `flux-image`, and a key issued for one is not
    /// entitled to the other. Before this field existed there was a single
    /// global default (`gpt-image-1`) plus an undocumented `OPENAI_IMAGE_MODEL`
    /// env escape hatch, so the built-in `image_generate` tool **failed by
    /// default for every FluxRouter user** — #310 fixed the endpoint and the
    /// key but not the model (measured live by lane `27-c3-media`: default arm
    /// `outcome: failed`, `OPENAI_IMAGE_MODEL=flux-image` arm succeeded).
    ///
    /// Per AGENTS.md's first rule this is a `ProviderCompat` question, never a
    /// `base_url.contains("flux")` conditional. Presets set it for the two
    /// providers that actually serve the endpoint (`openai_defaults`,
    /// `flux_router_defaults`); every other provider leaves it `None` and the
    /// image resolver falls back to its own global default.
    ///
    /// Override per account with `[compat] image_model = "dall-e-3"`. The
    /// `OPENAI_IMAGE_MODEL` env var still wins over this field, preserving the
    /// #265 escape hatch for anyone already relying on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
}

impl ProviderCompat {
    /// Defaults for Anthropic-family providers (Anthropic, Vertex)
    pub fn anthropic_defaults() -> Self {
        Self {
            ensure_alternation: Some(true),
            merge_same_role: Some(true),
            auto_tool_id: Some(true),
            // TODO(pricing-audit-2026-05-24): per-model thinking capability table —
            // Anthropic Opus 4.7 doesn't support extended thinking, but supports_thinking
            // is a flat provider flag. Needs a per-model lookup when that table exists.
            supports_thinking: Some(true),
            supports_effort: Some(false),
            cache_message_breakpoints: Some(true),
            // The same evidence that sets `cache_message_breakpoints` above:
            // this family honours explicit `cache_control` and prices cached
            // reads (`cost_per_cache_read_token` below). A warm session here
            // reading zero cached tokens is a break, not an absent feature.
            prompt_cache_expected: Some(true),
            provider_type: Some("anthropic".into()),
            // Per-PROVIDER (NOT per-model) Q2-2026 list price as a coarse default.
            // Every Anthropic model reports this price in TurnTrace.cost_usd
            // unless the user overrides via wcore.toml. Per-model pricing is
            // deferred to W6.1 (audit rev-2 finding 6).
            cost_per_input_token: Some(15.0 / 1_000_000.0),
            cost_per_output_token: Some(75.0 / 1_000_000.0),
            cost_per_cache_read_token: Some(1.5 / 1_000_000.0),
            cost_per_cache_write_token: Some(18.75 / 1_000_000.0),
            // Crucible #3: Anthropic chat models accept an explicit temperature.
            supports_temperature: Some(true),
            // Native Anthropic messages carry inline base64 image blocks.
            supports_vision: Some(true),
            supports_tools: Some(true),
            ..Default::default()
        }
    }

    /// Defaults for Bedrock (Anthropic + schema sanitization)
    pub fn bedrock_defaults() -> Self {
        Self {
            ensure_alternation: Some(true),
            merge_same_role: Some(true),
            auto_tool_id: Some(true),
            sanitize_schema: Some(true),
            supports_thinking: Some(true),
            supports_effort: Some(false),
            cache_message_breakpoints: Some(true),
            // The same evidence that sets `cache_message_breakpoints` above:
            // this family honours explicit `cache_control` and prices cached
            // reads (`cost_per_cache_read_token` below). A warm session here
            // reading zero cached tokens is a break, not an absent feature.
            prompt_cache_expected: Some(true),
            provider_type: Some("bedrock".into()),
            // Bedrock hosts Anthropic models; mirror the Anthropic list price.
            cost_per_input_token: Some(15.0 / 1_000_000.0),
            cost_per_output_token: Some(75.0 / 1_000_000.0),
            cost_per_cache_read_token: Some(1.5 / 1_000_000.0),
            cost_per_cache_write_token: Some(18.75 / 1_000_000.0),
            // Crucible #3: Anthropic-on-Bedrock chat models accept temperature.
            supports_temperature: Some(true),
            supports_tools: Some(true),
            ..Default::default()
        }
    }

    /// Defaults for Vertex (Anthropic via Google Cloud).
    ///
    /// Inherits the Anthropic behavioural flags and overrides only:
    /// - `provider_type` -> `"vertex"` for distinct cost/trace attribution.
    /// - `prompt_cache_expected` -> unset: whether Anthropic-on-Vertex serves
    ///   and reports prompt-cached input through this pipeline is unverified,
    ///   and the flag is an accusation (it makes the engine tell the user
    ///   their provider is re-billing an uncached prompt). Inheriting
    ///   Anthropic's `Some(true)` would make that accusation on evidence
    ///   gathered from a different endpoint. Same reason `minimax_defaults()`
    ///   clears `cache_message_breakpoints`.
    pub fn vertex_defaults() -> Self {
        Self {
            provider_type: Some("vertex".into()),
            prompt_cache_expected: None,
            ..Self::anthropic_defaults()
        }
    }

    /// Defaults for MiniMax via its Anthropic-compatible endpoint.
    ///
    /// MiniMax's `/anthropic` surface speaks the native Anthropic wire protocol
    /// (verified live 2026-06-18), so this inherits the Anthropic behavioural
    /// flags (`ensure_alternation`, `merge_same_role`, `auto_tool_id`,
    /// `supports_thinking`) and overrides only:
    /// - `provider_type` → `"minimax"` for distinct cost/trace attribution.
    /// - cost → `$0` sentinel (real-or-nothing: no published per-call price is
    ///   wired, so report an honest zero rather than a fabricated rate; users
    ///   override via `wcore.toml`). Mirrors the `openai_defaults()` sentinel.
    /// - `cache_message_breakpoints` → `false`: MiniMax's support for the
    ///   Anthropic prompt-caching beta is unverified (the factory also builds
    ///   this provider with caching off), so do not inject `cache_control`
    ///   blocks the endpoint may reject.
    /// - `prompt_cache_expected` → unset, for the same unverified reason: with
    ///   `cache_control` deliberately off there is no established capability
    ///   here, so never accuse this endpoint of a dead prompt cache.
    pub fn minimax_defaults() -> Self {
        Self {
            provider_type: Some("minimax".into()),
            cache_message_breakpoints: Some(false),
            prompt_cache_expected: None,
            cost_per_input_token: Some(0.0),
            cost_per_output_token: Some(0.0),
            cost_per_cache_read_token: None,
            cost_per_cache_write_token: None,
            // MiniMax runs two region-locked platforms with separate key
            // namespaces. The default `base_url` targets `api.minimax.io`; a key
            // issued on the other platform 401s there, so on a 401 retry the same
            // key against `api.minimaxi.com` and pin whichever authenticates.
            auth_fallback_base_url: Some("https://api.minimaxi.com/anthropic".into()),
            ..Self::anthropic_defaults()
        }
    }

    /// Defaults for native Google Gemini (Generative Language API).
    ///
    /// Distinct from `vertex_defaults()` — Vertex routes through the
    /// Anthropic-shape SSE pipeline (it hosts Claude); native Gemini uses
    /// its own request/response shape (functionDeclarations,
    /// systemInstruction, thoughtSignature). The compat flags below are
    /// the *behavioural* knobs the shared engine still asks about:
    ///
    /// - `merge_same_role`: Gemini tolerates either shape but prefers
    ///   merged turns; matches the body-builder in `gemini.rs`.
    /// - `cache_message_breakpoints`: Gemini doesn't honour explicit
    ///   message-level cache breakpoints (cf. compat.rs:68 comment).
    /// - `supports_thinking`: Gemini's `thinkingConfig.includeThoughts` is
    ///   reasoning-style (closer to OpenAI's `reasoning_effort`), not
    ///   Anthropic's `thinking_budget` — drive it through `reasoning_effort`.
    /// - `provider_type`: `"gemini"` so trace/cost attribution is distinct
    ///   from Vertex (which hosts Anthropic models on Google Cloud).
    pub fn gemini_defaults() -> Self {
        Self {
            merge_same_role: Some(true),
            ensure_alternation: Some(false),
            // Gemini has no `tool_use_id` on `functionCall` parts; the
            // engine synthesizes one in `parse_sse_chunk`. The auto-ID flag
            // is Anthropic-shape specific and would no-op here, but
            // keeping it false makes the intent explicit.
            auto_tool_id: Some(false),
            supports_thinking: Some(false),
            supports_effort: Some(true),
            effort_levels: Some(vec!["low".into(), "medium".into(), "high".into()]),
            cache_message_breakpoints: Some(false),
            provider_type: Some("gemini".into()),
            // Gemini's OpenAI-compat endpoint emits extra_content (the
            // google.thought routing marker) and tolerates its round-trip, so
            // it is the one provider that keeps emitting it outbound
            // (wayland-core#120). Every other provider strips it (default).
            emit_tool_call_extra_content: Some(true),
            // Q2-2026 Gemini 2.5 Pro list price (per Google AI Studio pricing page).
            // Free tier exists for low volume; the paid tier price is
            // $1.25 / 1M input tokens, $10 / 1M output. Use the paid
            // numbers as a coarse cost-attribution baseline — local runs
            // on the free tier overestimate by exactly this fraction,
            // which is the safe direction for the budget guardrail.
            cost_per_input_token: Some(1.25 / 1_000_000.0),
            cost_per_output_token: Some(10.0 / 1_000_000.0),
            cost_per_cache_read_token: Some(0.3125 / 1_000_000.0),
            cost_per_cache_write_token: None,
            // #112: Gemini's generateContent API defaults `maxOutputTokens` to
            // the served model's own ceiling when the field is absent, so an
            // unknown Gemini model with no explicit user cap may omit it.
            omit_max_tokens_when_unsized: Some(true),
            supports_vision: Some(true),
            supports_tools: Some(true),
            ..Default::default()
        }
    }

    /// Defaults for OpenAI-compatible providers
    pub fn openai_defaults() -> Self {
        Self {
            max_tokens_field: Some("max_tokens".into()),
            merge_assistant_messages: Some(true),
            clean_orphan_tool_calls: Some(true),
            dedup_tool_results: Some(true),
            supports_thinking: Some(false),
            supports_effort: Some(true),
            effort_levels: Some(vec!["low".into(), "medium".into(), "high".into()]),
            provider_type: Some("openai".into()),
            // OpenAI caches prompts automatically above its minimum
            // cacheable size and reports the hit under
            // `prompt_tokens_details.cached_tokens`, which `openai.rs` already
            // parses. The dead-cache probe only fires far above that minimum
            // (CACHE_DEAD_MIN_INPUT_TOKENS), so a small-prompt session cannot
            // trip it.
            prompt_cache_expected: Some(true),
            // Fix(pricing-audit-2026-05-24): was $8/$32 (GPT-5-class), which caused silent
            // 53x overcharge for every common OpenAI model not in the catalog (e.g. gpt-4o-mini).
            // Changed to $0/$0 sentinel — matches the openai_compat_provider() pattern.
            // Unmatched OpenAI models now report honest $0 instead of confident-but-wrong GPT-5 rate.
            // Common models (gpt-4o, gpt-4o-mini, gpt-4.1-mini, o1, o1-mini, o3-mini) are now in
            // the pricing.toml catalog so they resolve correctly before reaching this fallback.
            cost_per_input_token: Some(0.0),
            cost_per_output_token: Some(0.0),
            // #344/#359: OpenAI's hard tool-array limit is 128. Enforced
            // engine-side after MCP curation so MCP servers can't push past it.
            max_tools: Some(128),
            // Crucible #3: OpenAI chat models accept an explicit temperature;
            // the `o1*`/`o3*` reasoning families are excluded per-model by
            // `openai_compat::accepts_temperature`.
            supports_temperature: Some(true),
            // #648: native OpenAI hosts vision models (gpt-4o family).
            supports_vision: Some(true),
            supports_tools: Some(true),
            supports_structured_output: Some(true),
            // F-27C3-04: native OpenAI's `/v1/images/generations` model.
            // `dall-e-3` is region/tier-gated (#265); `gpt-image-1` is the
            // broadly-available one. Accounts that only have dall-e-3 override
            // with `[compat] image_model` or `OPENAI_IMAGE_MODEL`.
            image_model: Some("gpt-image-1".into()),
            ..Default::default()
        }
    }

    /// Defaults for an OpenAI-wire-compatible Tier-2 provider.
    ///
    /// v0.6.3 D.2 — the 6 new Tier-2 providers (Azure OpenAI, Together,
    /// Fireworks, Nvidia, Perplexity, Cerebras) all speak the OpenAI wire
    /// shape, so they share `openai_defaults()`'s behavioural flags
    /// (`merge_assistant_messages`, `clean_orphan_tool_calls`, etc.). But
    /// they are NOT OpenAI for the purposes of cost attribution: reusing
    /// `openai_defaults()` verbatim hard-codes `provider_type = "openai"`
    /// and GPT-class cost rows ($8/$32 per Mtok), which over-charges the
    /// budget tracker 10-40x for the cheap Llama-class models these
    /// providers host and mislabels every spend as `"openai"`.
    ///
    /// This helper takes the OpenAI behavioural preset, stamps the real
    /// provider id, and clears the inline cost rows. With the cost rows
    /// `None`, `wcore_observability::cost::estimate_turn_cost` returns
    /// `0.0` — an honest "unknown cost" — for any model not found in the
    /// `wcore-pricing` catalog. Per-model pricing comes from the catalog,
    /// keyed by `provider_type` (the real id), which now matches the
    /// `[<provider>.<model>]` rows in `pricing.toml`.
    pub(crate) fn openai_compat_provider(provider_id: &str) -> Self {
        // Server-side routing layers optimize input upstream; mark them
        // `"router"` so the engine defers client-side optimization passes.
        // Plain OpenAI-compat *providers* (Together, Groq, Deepseek, …) do NOT
        // route — they leave this `None` (→ "client"). This is the single,
        // vendor-neutral place that classifies a router vs. a direct provider.
        let input_optimization = match provider_id {
            // Sakana/Fugu is a multi-agent orchestration layer that routes and
            // optimizes upstream — same class as flux-router/openrouter.
            "flux-router" | "openrouter" | "sakana" => Some("router".to_string()),
            _ => None,
        };
        Self {
            provider_type: Some(provider_id.into()),
            input_optimization,
            // F-026 fix: use Some(0.0) as a sentinel meaning "pricing
            // resolves via catalog; emit cost events but report $0 when the
            // catalog has no entry for this model". Previously these were
            // `None`, which caused the bootstrap cost-attribution gate
            // (`bootstrap.rs:1093-1097`) to see `is_some() = false` and
            // never set `cost_attribution = true` — so OpenRouter, Groq,
            // Deepseek, xAI, and every other openai-compat secondary was
            // excluded from cost reporting even when session_cost would have
            // been emitted (F-009).
            //
            // The observability cost estimator already handles 0.0 as
            // "unknown / catalog-resolved" — this is not a regression.
            cost_per_input_token: Some(0.0),
            cost_per_output_token: Some(0.0),
            cost_per_cache_read_token: Some(0.0),
            cost_per_cache_write_token: Some(0.0),
            // #344/#359: OpenAI-wire routers/providers (ChatGPT, Azure,
            // flux-router, …) inherit the 128 tool-array hard cap.
            max_tools: Some(128),
            // F-27C3-04 — clear the inherited image model for the same reason
            // the cost rows are cleared: `openai_defaults()` declares OpenAI's
            // `gpt-image-1`, and an openai-compat provider is NOT OpenAI. Most
            // of this family is LLM-completion-only (Groq, Together, Deepseek,
            // …) and `openai_wire_media_base` already refuses to route media to
            // them, so the value would be unreachable — but inheriting another
            // vendor's model id is exactly the hardcoded-quirk shape this field
            // exists to remove. Providers that DO serve the endpoint declare it
            // explicitly (see `flux_router_defaults`).
            image_model: None,
            // Cleared for the same reason as `image_model` above: an
            // openai-compat provider is NOT OpenAI. `openai_defaults()` claims
            // an automatic prompt cache reported as
            // `prompt_tokens_details.cached_tokens`, which is a statement about
            // OpenAI's own endpoint. Inheriting it here would make the engine
            // accuse Groq, Together, Cerebras, Mistral, DeepSeek and every
            // other Tier-2 provider of silently re-billing an uncached prompt
            // on evidence gathered from a different vendor. Providers whose
            // capability IS established declare it explicitly (see
            // `flux_router_defaults`).
            prompt_cache_expected: None,
            ..Self::openai_defaults()
        }
    }

    /// Defaults for Azure OpenAI (OpenAI models hosted on Azure).
    /// Azure prices match OpenAI list price, but cost attribution must be
    /// labelled `"azure-openai"` and resolve against the catalog.
    pub fn azure_openai_defaults() -> Self {
        Self {
            // #648: Azure hosts the OpenAI vision models (gpt-4o family).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("azure-openai")
        }
    }

    /// Defaults for "Sign in with ChatGPT" (the Codex backend).
    ///
    /// Built on `openai_compat_provider("openai-chatgpt")` so cost attribution
    /// is labelled `"openai-chatgpt"` (catalog-resolved). Two Codex-specific
    /// overrides on top of the OpenAI-compat base:
    /// - `uses_responses_api = Some(true)`: the Codex backend speaks ONLY the
    ///   Responses wire format, so force it unconditionally rather than deferring
    ///   to the per-model family predicate.
    /// - `supports_effort` + `effort_levels`: Codex models are reasoning models
    ///   that accept `reasoning_effort`.
    ///
    /// NOTE: `max_output_tokens` is rejected by the Codex backend but is NOT
    /// suppressed here — the provider strips it from the request body directly
    /// (no new `ProviderCompat` field, so `merge()` is untouched).
    pub fn chatgpt_defaults() -> Self {
        Self {
            uses_responses_api: Some(true),
            supports_effort: Some(true),
            effort_levels: Some(vec!["low".into(), "medium".into(), "high".into()]),
            ..Self::openai_compat_provider("openai-chatgpt")
        }
    }

    /// Defaults for Together AI (open-weight model host).
    pub fn together_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to `/chat/completions` so the
        // native `--provider together` arm does not build `/v1/v1/...` (404).
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: Together hosts vision models (Llama-Vision, Qwen-VL).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("together")
        }
    }

    /// Defaults for Fireworks AI (open-weight model host).
    pub fn fireworks_defaults() -> Self {
        // Base URL ends in `/inference/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: Fireworks hosts vision models (Llama-Vision, Qwen-VL, Phi).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("fireworks")
        }
    }

    /// Defaults for Nvidia NIM / build.nvidia.com.
    pub fn nvidia_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: NVIDIA NIM catalog hosts vision models (Llama-Vision, NeVA).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("nvidia")
        }
    }

    /// Defaults for Perplexity (Sonar models).
    pub fn perplexity_defaults() -> Self {
        // Perplexity's endpoint is `https://api.perplexity.ai/chat/completions`
        // (no `/v1`); pin `api_path` so the default `/v1/chat/completions` does
        // not 404.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: Perplexity's Sonar chat API is text-only — omit images safely.
            supports_vision: Some(false),
            ..Self::openai_compat_provider("perplexity")
        }
    }

    /// Defaults for Cerebras (fast open-weight inference).
    pub fn cerebras_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: Cerebras serves text-only Llama models — omit images safely.
            supports_vision: Some(false),
            ..Self::openai_compat_provider("cerebras")
        }
    }

    /// Defaults for OpenRouter (100+ models via OpenAI-compat router surface).
    pub fn openrouter_defaults() -> Self {
        Self {
            // #112: OpenRouter applies the routed model's own ceiling when
            // `max_tokens` is absent, so an unknown/aliased model with no
            // explicit user cap may omit the field.
            omit_max_tokens_when_unsized: Some(true),
            // #648: OpenRouter routes hundreds of vision-capable models.
            supports_vision: Some(true),
            ..Self::openai_compat_provider("openrouter")
        }
    }

    /// Defaults for Flux Router (Sean's own OpenAI-compat router product).
    pub fn flux_router_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #112: Flux applies the served model's natural ceiling when
            // `max_tokens` is absent (the desktop #456/#462 contract), so a
            // tier alias / unknown served model with no explicit user cap may
            // omit the field. The sized internal budget still rides the
            // `x-wl-expected-output` header.
            omit_max_tokens_when_unsized: Some(true),
            // #648: Flux routes to vision-capable models across providers.
            supports_vision: Some(true),
            // F-27C3-04 — Flux serves `/v1/images/generations` under its OWN
            // model namespace. A Flux key is not entitled to `gpt-image-1`, so
            // before this the built-in `image_generate` tool failed for every
            // Flux user unless they knew about `OPENAI_IMAGE_MODEL=flux-image`.
            // Measured live by lane `27-c3-media`, one variable moved.
            image_model: Some("flux-image".into()),
            // #863 F2 — Flux is the endpoint that implements the loop-ownership
            // anti-collision handshake (Elevation is its server-side ladder), so
            // it is the one preset that opts in.
            flux_loop_provenance: Some(true),
            // #559: measured live against the real endpoint - a repeated
            // prefix reports `prompt_tokens_details.cached_tokens` 99.5% warm
            // from turn 2, and a real 112-round-trip session served 86% of its
            // input from cache. Flux caches server-side and reports it, so
            // zero cached tokens across a warm session is a defect worth
            // telling the user about. (Caching is automatic on the OpenAI
            // wire - this claims nothing about `cache_control`, which Flux
            // ignores; see `cache_message_breakpoints`, deliberately unset.)
            prompt_cache_expected: Some(true),
            ..Self::openai_compat_provider("flux-router")
        }
    }

    /// Defaults for Sakana AI ("Fugu") — OpenAI-compat orchestration endpoint
    /// at `https://api.sakana.ai/v1`. The base URL ends in `/v1`, so pin
    /// `api_path` to avoid `/v1/v1`. Classified as a router (Fugu optimizes
    /// upstream) via `openai_compat_provider("sakana")`.
    pub fn sakana_defaults() -> Self {
        Self {
            api_path: Some("/chat/completions".into()),
            ..Self::openai_compat_provider("sakana")
        }
    }

    /// v0.8.1 U10b — Defaults for DeepSeek (OpenAI-compatible chat surface).
    pub fn deepseek_defaults() -> Self {
        Self {
            // DeepSeek Reasoner 400s unless EVERY historical assistant message
            // carries `reasoning_content` once any turn produced thinking, so we
            // must replay prior-turn thinking here (finding #174 exception).
            replays_thinking_in_history: Some(true),
            // #648: DeepSeek chat/reasoner are text-only — omit images safely.
            supports_vision: Some(false),
            ..Self::openai_compat_provider("deepseek")
        }
    }

    /// v0.8.1 U10b — Defaults for xAI / Grok (OpenAI-compatible chat surface).
    pub fn xai_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // grok-4.3 (a reasoning model) 400s on the `stop` parameter, which
            // the engine otherwise attaches as a client-side output
            // optimization — suppress it so Grok models actually run.
            supports_stop_param: Some(false),
            // #648: xAI Grok hosts vision models (grok-2-vision, grok-4).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("xai")
        }
    }

    /// v0.8.1 U10b — Defaults for Groq (fast LPU inference, OpenAI-compatible).
    pub fn groq_defaults() -> Self {
        Self {
            // #648: Groq hosts multimodal Llama-4 (scout/maverick) vision models.
            supports_vision: Some(true),
            ..Self::openai_compat_provider("groq")
        }
    }

    /// Defaults for Moonshot (Kimi). v0.8.1 U10e.
    pub fn moonshot_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // Moonshot (Kimi) runs two region-locked platforms with separate key
            // namespaces, like MiniMax. The default base URL targets the
            // international host (`api.moonshot.ai`); a mainland-China key 401s
            // there, so on a 401 retry the same key against `api.moonshot.cn`.
            auth_fallback_base_url: Some("https://api.moonshot.cn/v1".into()),
            // Kimi mirrors DeepSeek's strict-reasoner contract: once any turn has
            // thinking, every historical assistant message must carry
            // `reasoning_content` or the request 400s — so replay it here.
            replays_thinking_in_history: Some(true),
            ..Self::openai_compat_provider("moonshot")
        }
    }

    /// Defaults for Alibaba Qwen via DashScope's OpenAI-compat mode.
    /// v0.8.1 U10e.
    pub fn qwen_defaults() -> Self {
        // Base URL ends in `/compatible-mode/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: DashScope hosts the Qwen-VL vision family (qwen-vl-max).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("qwen")
        }
    }

    /// Defaults for Mistral AI (OpenAI-compatible chat surface).
    /// F-025 fix: wired from orphan module to reachable ProviderType arm.
    pub fn mistral_defaults() -> Self {
        // Base URL ends in `/v1`; pin `api_path` to avoid `/v1/v1`.
        Self {
            api_path: Some("/chat/completions".into()),
            // #648: Mistral hosts the Pixtral vision family (pixtral-large).
            supports_vision: Some(true),
            ..Self::openai_compat_provider("mistral")
        }
    }

    /// Defaults for Cohere (native chat API, not OpenAI-compat).
    /// F-025 fix: wired from orphan module to reachable ProviderType arm.
    /// Cohere's native API is not OpenAI-wire-compatible; pricing resolves
    /// via catalog keyed by `provider_type = "cohere"`.
    pub fn cohere_defaults() -> Self {
        Self {
            provider_type: Some("cohere".into()),
            cost_per_input_token: None,
            cost_per_output_token: None,
            cost_per_cache_read_token: None,
            cost_per_cache_write_token: None,
            ..Default::default()
        }
    }

    /// Defaults for Ollama (local provider — pricing is zero).
    /// Not currently routed via `ProviderType` (only Anthropic/OpenAI/Bedrock/
    /// Vertex are wired through that enum); exposed so users with an Ollama
    /// alias in wcore.toml can opt in via explicit compat, and so the cost
    /// helper has a baseline "local = free" preset to test against.
    pub fn ollama_defaults() -> Self {
        Self {
            provider_type: Some("ollama".into()),
            cost_per_input_token: Some(0.0),
            cost_per_output_token: Some(0.0),
            cost_per_cache_read_token: Some(0.0),
            cost_per_cache_write_token: Some(0.0),
            cost_is_known_free: Some(true),
            ..Default::default()
        }
    }

    /// Merge user config over defaults (user wins on non-None fields)
    pub fn merge(defaults: Self, user: Self) -> Self {
        // Per-axis cost-rate provenance. A rate the user states here is
        // authoritative; a rate inherited from `defaults` is authoritative
        // only if `defaults` itself was user-sourced (profile-over-profile
        // merges), which a built-in preset never is.
        let axis = |user_rate: Option<f64>, default_rate: Option<f64>, inherited: bool| {
            match (user_rate, default_rate) {
                (Some(_), _) => true,
                (None, Some(_)) => inherited,
                // No rate on this axis at all: nothing was fabricated, so the
                // axis cannot taint the profile. Whether the *turn* can be
                // priced without it is the cost helper's decision, not this
                // one.
                (None, None) => true,
            }
        };
        let prior = defaults.cost_rate_provenance;
        let cost_rate_provenance = CostRateProvenance {
            input: axis(
                user.cost_per_input_token,
                defaults.cost_per_input_token,
                prior.input,
            ),
            output: axis(
                user.cost_per_output_token,
                defaults.cost_per_output_token,
                prior.output,
            ),
            cache_read: axis(
                user.cost_per_cache_read_token,
                defaults.cost_per_cache_read_token,
                prior.cache_read,
            ),
            cache_write: axis(
                user.cost_per_cache_write_token,
                defaults.cost_per_cache_write_token,
                prior.cache_write,
            ),
        };
        Self {
            max_tokens_field: user.max_tokens_field.or(defaults.max_tokens_field),
            read_timeout_ms: user.read_timeout_ms.or(defaults.read_timeout_ms),
            merge_assistant_messages: user
                .merge_assistant_messages
                .or(defaults.merge_assistant_messages),
            clean_orphan_tool_calls: user
                .clean_orphan_tool_calls
                .or(defaults.clean_orphan_tool_calls),
            dedup_tool_results: user.dedup_tool_results.or(defaults.dedup_tool_results),
            ensure_alternation: user.ensure_alternation.or(defaults.ensure_alternation),
            merge_same_role: user.merge_same_role.or(defaults.merge_same_role),
            sanitize_schema: user.sanitize_schema.or(defaults.sanitize_schema),
            strip_patterns: user.strip_patterns.or(defaults.strip_patterns),
            auto_tool_id: user.auto_tool_id.or(defaults.auto_tool_id),
            api_path: user.api_path.or(defaults.api_path),
            supports_thinking: user.supports_thinking.or(defaults.supports_thinking),
            supports_effort: user.supports_effort.or(defaults.supports_effort),
            effort_levels: user.effort_levels.or(defaults.effort_levels),
            cache_message_breakpoints: user
                .cache_message_breakpoints
                .or(defaults.cache_message_breakpoints),
            prompt_cache_expected: user
                .prompt_cache_expected
                .or(defaults.prompt_cache_expected),
            provider_type: user.provider_type.or(defaults.provider_type),
            cost_per_input_token: user.cost_per_input_token.or(defaults.cost_per_input_token),
            cost_per_output_token: user
                .cost_per_output_token
                .or(defaults.cost_per_output_token),
            cost_per_cache_read_token: user
                .cost_per_cache_read_token
                .or(defaults.cost_per_cache_read_token),
            cost_per_cache_write_token: user
                .cost_per_cache_write_token
                .or(defaults.cost_per_cache_write_token),
            cost_is_known_free: user.cost_is_known_free.or(defaults.cost_is_known_free),
            cost_rate_provenance,
            input_optimization: user.input_optimization.or(defaults.input_optimization),
            compact_bash: user.compact_bash.or(defaults.compact_bash),
            include_usage_in_stream: user
                .include_usage_in_stream
                .or(defaults.include_usage_in_stream),
            uses_responses_api: user.uses_responses_api.or(defaults.uses_responses_api),
            uses_max_completion_tokens: user
                .uses_max_completion_tokens
                .or(defaults.uses_max_completion_tokens),
            azure_auth_mode: user.azure_auth_mode.or(defaults.azure_auth_mode),
            auth_fallback_base_url: user
                .auth_fallback_base_url
                .or(defaults.auth_fallback_base_url),
            supports_stop_param: user.supports_stop_param.or(defaults.supports_stop_param),
            replays_thinking_in_history: user
                .replays_thinking_in_history
                .or(defaults.replays_thinking_in_history),
            emit_tool_call_extra_content: user
                .emit_tool_call_extra_content
                .or(defaults.emit_tool_call_extra_content),
            tier_models: user.tier_models.or(defaults.tier_models),
            max_tools: user.max_tools.or(defaults.max_tools),
            // Crucible #3 — merge ripple: a new compat field MUST be threaded
            // here or it is silently dropped when user config is merged over the
            // provider preset.
            supports_temperature: user.supports_temperature.or(defaults.supports_temperature),
            omit_max_tokens_when_unsized: user
                .omit_max_tokens_when_unsized
                .or(defaults.omit_max_tokens_when_unsized),
            // #863 F2 — must be merged like every other field. Omitting it here
            // would drop the `flux_router_defaults` opt-in on every merged
            // compat, leaving the loop-provenance gate permanently false in
            // production while its unit tests (which read the preset directly)
            // stayed green.
            flux_loop_provenance: user.flux_loop_provenance.or(defaults.flux_loop_provenance),
            supports_vision: user.supports_vision.or(defaults.supports_vision),
            supports_tools: user.supports_tools.or(defaults.supports_tools),
            supports_structured_output: user
                .supports_structured_output
                .or(defaults.supports_structured_output),
            // F-27C3-04 — see the Crucible #3 note above: threading a new field
            // here is not optional. Without this arm `[compat] image_model` in
            // wcore.toml is silently discarded and the preset always wins.
            image_model: user.image_model.or(defaults.image_model),
        }
    }

    /// Finding #174: resolve the configured tier-substitution model for a
    /// routing-tier label (`"cheap"` / `"balanced"` / `"premium"`).
    ///
    /// Returns `None` when (a) no `[compat.tier_models]` map is configured —
    /// the default, i.e. the feature is OFF, (b) the map has no entry for this
    /// tier, or (c) the tier is `premium` (never downgraded). When this returns
    /// `Some`, the engine swaps `LlmRequest::model` to it for the turn.
    pub fn tier_model(&self, tier: &str) -> Option<&str> {
        self.tier_models.as_ref()?.model_for_tier(tier)
    }

    // --- Resolved accessors (Option<bool> → bool with false default) ---

    pub fn merge_assistant_messages(&self) -> bool {
        self.merge_assistant_messages.unwrap_or(false)
    }

    pub fn clean_orphan_tool_calls(&self) -> bool {
        self.clean_orphan_tool_calls.unwrap_or(false)
    }

    pub fn dedup_tool_results(&self) -> bool {
        self.dedup_tool_results.unwrap_or(false)
    }

    pub fn ensure_alternation(&self) -> bool {
        self.ensure_alternation.unwrap_or(false)
    }

    pub fn merge_same_role(&self) -> bool {
        self.merge_same_role.unwrap_or(false)
    }

    pub fn sanitize_schema(&self) -> bool {
        self.sanitize_schema.unwrap_or(false)
    }

    pub fn auto_tool_id(&self) -> bool {
        self.auto_tool_id.unwrap_or(false)
    }

    pub fn api_path(&self) -> &str {
        self.api_path.as_deref().unwrap_or("/v1/chat/completions")
    }

    /// Whether to send the OpenAI `stop` parameter. Defaults to `true`; xAI
    /// sets it `false` because `grok-4.3` (a reasoning model) 400s on `stop`.
    pub fn supports_stop_param(&self) -> bool {
        self.supports_stop_param.unwrap_or(true)
    }

    /// Crucible #3: whether to emit an explicit `temperature` body field.
    /// Defaults to `true` (chat models accept it). `Some(false)` suppresses it
    /// for endpoints that reject the parameter. The per-model `o1*`/`o3*`
    /// exclusion is layered on top via `openai_compat::accepts_temperature`.
    pub fn supports_temperature(&self) -> bool {
        self.supports_temperature.unwrap_or(true)
    }

    /// #112: whether the provider tolerates omitting the max-tokens wire field
    /// for a model with no registry-known output ceiling. Defaults to `false`
    /// (always send a sized value); the gemini / openrouter / flux-router
    /// presets set `true`.
    pub fn omit_max_tokens_when_unsized(&self) -> bool {
        self.omit_max_tokens_when_unsized.unwrap_or(false)
    }

    /// #863 F2 — whether this endpoint speaks the Flux loop-ownership
    /// handshake. Default `false`: absent an explicit opt-in, no
    /// loop-provenance field is ever put on the wire.
    pub fn flux_loop_provenance(&self) -> bool {
        self.flux_loop_provenance.unwrap_or(false)
    }

    /// #648: whether to send inline images as OpenAI `image_url` multipart
    /// parts. Defaults to `false` — a `ContentBlock::Image` is replaced with the
    /// shared text placeholder for text-only providers rather than risking a
    /// 400. Vision-capable presets (openai, azure-openai, openrouter, together,
    /// fireworks, nvidia, xai, qwen, groq, flux-router, mistral) set `true`.
    pub fn supports_vision(&self) -> bool {
        self.supports_vision.unwrap_or(false)
    }

    pub fn supports_tools(&self) -> bool {
        self.supports_tools.unwrap_or(false)
    }

    pub fn supports_structured_output(&self) -> bool {
        self.supports_structured_output.unwrap_or(false)
    }

    /// Whether to replay historical assistant `reasoning_content` on the Chat
    /// Completions path. Defaults to `false`: historical thinking is dropped at
    /// the wire (no recurring input billing, matching Anthropic — finding #174).
    /// DeepSeek/Moonshot set `true` because their API 400s without the replay.
    pub fn replays_thinking_in_history(&self) -> bool {
        self.replays_thinking_in_history.unwrap_or(false)
    }

    /// Whether to re-serialize internal `extra_content` onto outbound
    /// `tool_calls`. Defaults to `false` (strip): only Google/Gemini opts in.
    /// See [`ProviderCompat::emit_tool_call_extra_content`] (wayland-core#120).
    pub fn emit_tool_call_extra_content(&self) -> bool {
        self.emit_tool_call_extra_content.unwrap_or(false)
    }

    pub fn supports_thinking(&self) -> bool {
        self.supports_thinking.unwrap_or(false)
    }

    pub fn supports_effort(&self) -> bool {
        self.supports_effort.unwrap_or(false)
    }

    pub fn effort_levels(&self) -> &[String] {
        self.effort_levels.as_deref().unwrap_or(&[])
    }

    /// Resolved accessor for `cache_message_breakpoints`. None → false.
    pub fn cache_message_breakpoints(&self) -> bool {
        self.cache_message_breakpoints.unwrap_or(false)
    }

    /// Resolved accessor for `prompt_cache_expected`. None → false, so an
    /// unknown endpoint is never reported as having a broken prompt cache.
    pub fn prompt_cache_expected(&self) -> bool {
        self.prompt_cache_expected.unwrap_or(false)
    }

    /// W6 — structured provider identity. Defaults to `"unknown"` when not set.
    /// Populated by every preset; consumed by `wcore-agent::engine` for
    /// `TurnTrace.provider` and by `wcore-observability::cost::estimate_turn_cost`.
    pub fn provider_type(&self) -> &str {
        self.provider_type.as_deref().unwrap_or("unknown")
    }

    /// Resolved input-optimization capability. `"router"` means the endpoint
    /// optimizes input server-side (defer client-side passes); `"client"`
    /// (the default when unset) means the client must optimize itself.
    pub fn input_optimization(&self) -> &str {
        self.input_optimization.as_deref().unwrap_or("client")
    }

    /// Resolved gate for native Bash output compaction. Defaults ON: verbose
    /// cargo/git/test/grep output is compacted before reaching the model's
    /// transcript unless a provider/profile sets `compact_bash = false`.
    pub fn compact_bash(&self) -> bool {
        self.compact_bash.unwrap_or(true)
    }

    /// Resolved gate for `stream_options: {include_usage: true}`. Defaults ON;
    /// set `include_usage_in_stream = false` for generic OpenAI-compatible
    /// endpoints that 400 on the field (FerroxLabs/wayland#86).
    pub fn include_usage_in_stream(&self) -> bool {
        self.include_usage_in_stream.unwrap_or(true)
    }

    /// Optional override for the OpenAI chat-vs-responses API surface.
    /// `None` (default) defers to the per-model family predicate
    /// (`wcore_providers::openai_compat::model_uses_responses_api`).
    pub fn uses_responses_api(&self) -> Option<bool> {
        self.uses_responses_api
    }

    /// F27: optional override for whether the request body uses
    /// `max_completion_tokens` instead of `max_tokens`. `None` (default)
    /// defers to the per-model family prefix heuristic
    /// (`wcore_providers::openai_compat::wants_max_completion_tokens`).
    pub fn uses_max_completion_tokens(&self) -> Option<bool> {
        self.uses_max_completion_tokens
    }

    /// #344/#359: the provider's hard cap on the number of tools per request.
    /// `None` (the default for non-OpenAI wire protocols) means no cap.
    pub fn max_tools(&self) -> Option<usize> {
        self.max_tools
    }
}

/// Sanitize a JSON Schema for strict providers (e.g., Bedrock).
/// - Root type must be "object" (wrap if not)
/// - Recursively remove "additionalProperties"
/// - Normalize array types: ["string", "null"] → "string"
pub fn sanitize_json_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();

    // Ensure root type is "object"
    if schema.get("type").and_then(|t| t.as_str()) != Some("object") {
        schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": schema
            },
            "required": ["value"]
        });
    }

    strip_additional_properties(&mut schema);
    normalize_array_types(&mut schema);
    schema
}

fn strip_additional_properties(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        obj.remove("additionalProperties");
        for v in obj.values_mut() {
            strip_additional_properties(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            strip_additional_properties(v);
        }
    }
}

fn normalize_array_types(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        // Normalize ["string", "null"] → "string"
        if let Some(arr) = obj.get("type").and_then(Value::as_array) {
            let non_null: Vec<&Value> = arr.iter().filter(|v| v.as_str() != Some("null")).collect();
            if non_null.len() == 1 {
                obj.insert("type".to_string(), non_null[0].clone());
            }
        }
        for v in obj.values_mut() {
            normalize_array_types(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            normalize_array_types(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_anthropic_defaults() {
        let compat = ProviderCompat::anthropic_defaults();
        assert!(compat.ensure_alternation());
        assert!(compat.merge_same_role());
        assert!(compat.auto_tool_id());
        assert!(!compat.sanitize_schema());
        assert!(!compat.merge_assistant_messages());
        assert!(!compat.clean_orphan_tool_calls());
    }

    #[test]
    fn test_minimax_defaults() {
        let compat = ProviderCompat::minimax_defaults();
        // Inherits the Anthropic-wire behavioural flags...
        assert!(compat.ensure_alternation());
        assert!(compat.merge_same_role());
        assert!(compat.auto_tool_id());
        assert!(compat.supports_thinking());
        // ...but is attributed to MiniMax, not Anthropic.
        assert_eq!(compat.provider_type(), "minimax");
        // Caching off (unverified) and real-or-nothing $0 cost sentinel — NOT
        // the Anthropic list price, which would mis-bill every MiniMax call.
        assert!(!compat.cache_message_breakpoints());
        assert_eq!(compat.cost_per_input_token, Some(0.0));
        assert_eq!(compat.cost_per_output_token, Some(0.0));
        assert_eq!(compat.cost_per_cache_read_token, None);
        // Region-locked-key failover: a 401 on the default `api.minimax.io`
        // host retries `api.minimaxi.com` so a key from either MiniMax platform
        // works without the user knowing which region issued it.
        assert_eq!(
            compat.auth_fallback_base_url.as_deref(),
            Some("https://api.minimaxi.com/anthropic")
        );
    }

    #[test]
    fn test_bedrock_defaults() {
        let compat = ProviderCompat::bedrock_defaults();
        assert!(compat.ensure_alternation());
        assert!(compat.merge_same_role());
        assert!(compat.auto_tool_id());
        assert!(compat.sanitize_schema());
    }

    #[test]
    fn test_openai_defaults() {
        let compat = ProviderCompat::openai_defaults();
        assert!(compat.merge_assistant_messages());
        assert!(compat.clean_orphan_tool_calls());
        assert!(compat.dedup_tool_results());
        assert_eq!(compat.max_tokens_field.as_deref(), Some("max_tokens"));
        assert!(!compat.ensure_alternation());
    }

    /// Regression guard (2026-06 provider-correctness audit): native-arm
    /// providers whose base URL already ends in `/v1` (together, fireworks,
    /// nvidia, cerebras, flux-router, xai, mistral, moonshot, qwen) — or whose
    /// vendor endpoint omits `/v1` entirely (perplexity) — must pin `api_path` to
    /// `/chat/completions`. Otherwise the default `/v1/chat/completions`
    /// produces `…/v1/v1/chat/completions` (or an erroneous `/v1`) and every
    /// request 404s out of the box.
    #[test]
    fn openai_compat_v1_base_providers_pin_api_path() {
        for compat in [
            ProviderCompat::together_defaults(),
            ProviderCompat::fireworks_defaults(),
            ProviderCompat::nvidia_defaults(),
            ProviderCompat::perplexity_defaults(),
            ProviderCompat::cerebras_defaults(),
            ProviderCompat::flux_router_defaults(),
            ProviderCompat::xai_defaults(),
            ProviderCompat::mistral_defaults(),
            ProviderCompat::moonshot_defaults(),
            ProviderCompat::qwen_defaults(),
        ] {
            assert_eq!(compat.api_path(), "/chat/completions");
        }
    }

    #[test]
    fn xai_suppresses_stop_param_but_others_keep_it() {
        // grok-4.3 400s on `stop`, so xAI must report supports_stop_param=false;
        // every other provider keeps the default true (engine still attaches the
        // fluff-stop output optimization on client-optimized routes).
        assert!(
            !ProviderCompat::xai_defaults().supports_stop_param(),
            "xAI must suppress the stop parameter (grok-4.3 rejects it)"
        );
        assert!(ProviderCompat::openai_defaults().supports_stop_param());
        assert!(ProviderCompat::anthropic_defaults().supports_stop_param());
        assert!(ProviderCompat::groq_defaults().supports_stop_param());
    }

    #[test]
    fn dual_region_providers_set_auth_fallback() {
        // Moonshot (Kimi) and MiniMax both run two region-locked platforms with
        // separate key namespaces, so a key from the other region 401s on the
        // default host and must fail over.
        assert_eq!(
            ProviderCompat::moonshot_defaults()
                .auth_fallback_base_url
                .as_deref(),
            Some("https://api.moonshot.cn/v1")
        );
        assert_eq!(
            ProviderCompat::minimax_defaults()
                .auth_fallback_base_url
                .as_deref(),
            Some("https://api.minimaxi.com/anthropic")
        );
        // Single-region providers leave it unset.
        assert!(
            ProviderCompat::openai_defaults()
                .auth_fallback_base_url
                .is_none()
        );
    }

    #[test]
    fn test_merge_user_overrides_defaults() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            max_tokens_field: Some("max_completion_tokens".into()),
            merge_assistant_messages: Some(false),
            ..Default::default()
        };

        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(
            merged.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert!(!merged.merge_assistant_messages());
        // Non-overridden fields keep defaults
        assert!(merged.clean_orphan_tool_calls());
        assert!(merged.dedup_tool_results());
    }

    #[test]
    fn test_merge_empty_user_keeps_defaults() {
        let defaults = ProviderCompat::anthropic_defaults();
        let user = ProviderCompat::default();

        let merged = ProviderCompat::merge(defaults, user);
        assert!(merged.ensure_alternation());
        assert!(merged.merge_same_role());
        assert!(merged.auto_tool_id());
    }

    #[test]
    fn test_merge_user_read_timeout_overrides_default() {
        let defaults = ProviderCompat {
            read_timeout_ms: std::num::NonZeroU64::new(300_000),
            ..ProviderCompat::openai_defaults()
        };
        let user = ProviderCompat {
            read_timeout_ms: std::num::NonZeroU64::new(75),
            ..ProviderCompat::default()
        };

        assert_eq!(
            ProviderCompat::merge(defaults, user)
                .read_timeout_ms
                .map(std::num::NonZeroU64::get),
            Some(75)
        );
    }

    #[test]
    fn read_timeout_rejects_zero() {
        let parsed = serde_json::from_value::<ProviderCompat>(serde_json::json!({
            "read_timeout_ms": 0
        }));
        assert!(parsed.is_err(), "a zero read timeout must fail closed");
    }

    #[test]
    fn test_sanitize_schema_wraps_non_object_root() {
        let schema = json!({"type": "string"});
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["type"], "object");
        assert_eq!(sanitized["properties"]["value"]["type"], "string");
    }

    #[test]
    fn test_sanitize_schema_removes_additional_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "additionalProperties": false}
            },
            "additionalProperties": false
        });
        let sanitized = sanitize_json_schema(&schema);

        assert!(sanitized.get("additionalProperties").is_none());
        assert!(
            sanitized["properties"]["name"]
                .get("additionalProperties")
                .is_none()
        );
    }

    #[test]
    fn test_sanitize_schema_normalizes_array_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": ["string", "null"]}
            }
        });
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_sanitize_schema_no_change_for_valid_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": {"type": "string"}
            },
            "required": ["cmd"]
        });
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["type"], "object");
        assert_eq!(sanitized["properties"]["cmd"]["type"], "string");
    }

    #[test]
    fn test_anthropic_defaults_capability_fields() {
        let compat = ProviderCompat::anthropic_defaults();
        assert_eq!(compat.supports_thinking, Some(true));
        assert_eq!(compat.supports_effort, Some(false));
        assert!(compat.effort_levels.is_none());
    }

    #[test]
    fn test_openai_defaults_capability_fields() {
        let compat = ProviderCompat::openai_defaults();
        assert_eq!(compat.supports_thinking, Some(false));
        assert_eq!(compat.supports_effort, Some(true));
        assert_eq!(
            compat.effort_levels,
            Some(vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string()
            ])
        );
    }

    #[test]
    fn test_bedrock_defaults_capability_fields() {
        let compat = ProviderCompat::bedrock_defaults();
        assert_eq!(compat.supports_thinking, Some(true));
        assert_eq!(compat.supports_effort, Some(false));
    }

    #[test]
    fn test_merge_capability_fields_user_overrides() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            supports_thinking: Some(true),
            ..Default::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.supports_thinking, Some(true));
        assert_eq!(merged.supports_effort, Some(true));
    }

    #[test]
    fn test_capability_accessors() {
        let compat = ProviderCompat::anthropic_defaults();
        assert!(compat.supports_thinking());
        assert!(!compat.supports_effort());
        assert!(compat.effort_levels().is_empty());

        let compat2 = ProviderCompat::openai_defaults();
        assert!(!compat2.supports_thinking());
        assert!(compat2.supports_effort());
        assert_eq!(compat2.effort_levels(), &["low", "medium", "high"]);
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"
max_tokens_field = "max_completion_tokens"
merge_assistant_messages = true
strip_patterns = ["__REASONING__"]
"#;
        let compat: ProviderCompat = toml::from_str(toml_str).unwrap();
        assert_eq!(
            compat.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert_eq!(compat.merge_assistant_messages, Some(true));
        assert_eq!(
            compat.strip_patterns,
            Some(vec!["__REASONING__".to_string()])
        );
        assert!(compat.clean_orphan_tool_calls.is_none());
    }

    /// R77: `azure_auth_mode` is now a real, honored config field (previously
    /// the `AzureAuthMode` enum existed but was wired into no struct, so a
    /// user's `auth_mode` setting was silently ignored).
    #[test]
    fn azure_auth_mode_deserializes_and_merges() {
        use crate::config::AzureAuthMode;

        // kebab-case TOML value parses to the enum.
        let compat: ProviderCompat = toml::from_str("azure_auth_mode = \"aad-bearer\"").unwrap();
        assert_eq!(compat.azure_auth_mode, Some(AzureAuthMode::AadBearer));

        // Absent => None (resolves to the api-key default at bootstrap).
        let bare: ProviderCompat = toml::from_str("max_tokens_field = \"x\"").unwrap();
        assert_eq!(bare.azure_auth_mode, None);

        // merge: an explicit user override wins over the preset default.
        let defaults = ProviderCompat {
            azure_auth_mode: Some(AzureAuthMode::ApiKey),
            ..Default::default()
        };
        let user = ProviderCompat {
            azure_auth_mode: Some(AzureAuthMode::AadBearer),
            ..Default::default()
        };
        assert_eq!(
            ProviderCompat::merge(defaults, user).azure_auth_mode,
            Some(AzureAuthMode::AadBearer)
        );
    }
}

// --- W1 Task 3: cache_message_breakpoints ---

#[cfg(test)]
mod cache_breakpoint_tests {
    use super::*;

    #[test]
    fn anthropic_defaults_enable_cache_message_breakpoints() {
        let compat = ProviderCompat::anthropic_defaults();
        assert_eq!(compat.cache_message_breakpoints, Some(true));
        assert!(compat.cache_message_breakpoints());
    }

    #[test]
    fn bedrock_defaults_enable_cache_message_breakpoints() {
        let compat = ProviderCompat::bedrock_defaults();
        assert_eq!(compat.cache_message_breakpoints, Some(true));
        assert!(compat.cache_message_breakpoints());
    }

    #[test]
    fn chatgpt_defaults_force_responses_and_tag_provider() {
        let c = ProviderCompat::chatgpt_defaults();
        // Codex speaks only the Responses wire format — forced unconditionally.
        assert_eq!(c.uses_responses_api(), Some(true));
        // Cost attribution carries the real provider id, not "openai".
        assert_eq!(c.provider_type(), "openai-chatgpt");
        // Reasoning-effort capability is advertised.
        assert_eq!(c.supports_effort, Some(true));
        assert_eq!(
            c.effort_levels,
            Some(vec!["low".into(), "medium".into(), "high".into()])
        );
    }

    #[test]
    fn openai_defaults_do_not_enable_cache_message_breakpoints() {
        let compat = ProviderCompat::openai_defaults();
        // None or Some(false) both resolve to false through the accessor —
        // we leave it None to preserve "use provider-type default" semantics
        // for OpenAI users who haven't asked for it.
        assert_eq!(compat.cache_message_breakpoints, None);
        assert!(!compat.cache_message_breakpoints());
    }

    #[test]
    fn user_can_override_cache_message_breakpoints_via_merge() {
        let defaults = ProviderCompat::anthropic_defaults();
        let user = ProviderCompat {
            cache_message_breakpoints: Some(false),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.cache_message_breakpoints, Some(false));
        assert!(!merged.cache_message_breakpoints());
    }

    #[test]
    fn cache_message_breakpoints_accessor_returns_false_when_none() {
        let compat = ProviderCompat::default();
        assert_eq!(compat.cache_message_breakpoints, None);
        assert!(!compat.cache_message_breakpoints());
    }

    #[test]
    fn vertex_provider_type_inherits_anthropic_cache_breakpoints() {
        // Asserts the resolution at wcore-config/src/config.rs:400:
        //   ProviderType::Vertex => ProviderCompat::anthropic_defaults()
        // is exercised by the match-arm code path. We assert the
        // observable contract (cache_message_breakpoints() returns true for
        // a Vertex-resolved compat) rather than the match itself so the test
        // survives any future renaming of the preset constructor.
        //
        // If a future Vertex-specific preset is introduced and silently
        // drops the cache marker, this assertion fails — exactly the
        // "no hardcoded provider quirks" failure mode AGENTS.md warns
        // about.
        use crate::config::ProviderType;

        let resolved = match ProviderType::Vertex {
            ProviderType::Anthropic => ProviderCompat::anthropic_defaults(),
            ProviderType::Bedrock => ProviderCompat::bedrock_defaults(),
            ProviderType::Vertex => ProviderCompat::vertex_defaults(),
            ProviderType::Gemini => ProviderCompat::gemini_defaults(),
            ProviderType::OpenAI => ProviderCompat::openai_defaults(),
            ProviderType::AzureOpenAI => ProviderCompat::azure_openai_defaults(),
            ProviderType::Together => ProviderCompat::together_defaults(),
            ProviderType::Fireworks => ProviderCompat::fireworks_defaults(),
            ProviderType::Nvidia => ProviderCompat::nvidia_defaults(),
            ProviderType::Perplexity => ProviderCompat::perplexity_defaults(),
            ProviderType::Cerebras => ProviderCompat::cerebras_defaults(),
            ProviderType::OpenRouter => ProviderCompat::openrouter_defaults(),
            ProviderType::FluxRouter => ProviderCompat::flux_router_defaults(),
            ProviderType::Deepseek => ProviderCompat::deepseek_defaults(),
            ProviderType::Xai => ProviderCompat::xai_defaults(),
            ProviderType::Groq => ProviderCompat::groq_defaults(),
            ProviderType::Moonshot => ProviderCompat::moonshot_defaults(),
            ProviderType::Qwen => ProviderCompat::qwen_defaults(),
            // F-025: Mistral + Cohere arms added to keep this exhaustive match
            // compiling as the ProviderType enum grows.
            ProviderType::Mistral => ProviderCompat::mistral_defaults(),
            ProviderType::Cohere => ProviderCompat::cohere_defaults(),
            ProviderType::OpenAIChatGpt => ProviderCompat::chatgpt_defaults(),
            ProviderType::MiniMax => ProviderCompat::minimax_defaults(),
            ProviderType::Sakana => ProviderCompat::sakana_defaults(),
        };
        assert_eq!(
            resolved.cache_message_breakpoints,
            Some(true),
            "Vertex must inherit cache_message_breakpoints from anthropic_defaults \
             (see config.rs:400). If this fails, either a vertex_defaults() preset \
             was introduced — in which case set cache_message_breakpoints: Some(true) \
             on it — or the inheritance match arm changed."
        );
        assert!(resolved.cache_message_breakpoints());
    }
}

// --- W6 T1: provider_type + cost rows ---

#[cfg(test)]
mod w6_provider_type_and_cost_tests {
    use super::*;

    #[test]
    fn every_default_preset_has_provider_type() {
        assert_eq!(
            ProviderCompat::anthropic_defaults().provider_type(),
            "anthropic"
        );
        assert_eq!(
            ProviderCompat::bedrock_defaults().provider_type(),
            "bedrock"
        );
        assert_eq!(ProviderCompat::openai_defaults().provider_type(), "openai");
        assert_eq!(ProviderCompat::vertex_defaults().provider_type(), "vertex");
        assert_eq!(ProviderCompat::ollama_defaults().provider_type(), "ollama");
    }

    /// Finding #174: with no `[compat.tier_models]` configured, the accessor
    /// returns `None` for every tier — proving the feature is OFF by default and
    /// the engine performs no swap (default behaviour unchanged).
    #[test]
    fn tier_model_is_none_by_default() {
        let c = ProviderCompat::anthropic_defaults();
        assert!(c.tier_models.is_none());
        assert_eq!(c.tier_model("cheap"), None);
        assert_eq!(c.tier_model("balanced"), None);
        assert_eq!(c.tier_model("premium"), None);
    }

    /// A configured cheap model resolves; an unconfigured balanced tier stays
    /// `None` (per-tier opt-in).
    #[test]
    fn tier_model_resolves_configured_cheap_only() {
        let c = ProviderCompat {
            tier_models: Some(TierModels {
                cheap: Some("haiku".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(c.tier_model("cheap"), Some("haiku"));
        assert_eq!(c.tier_model("balanced"), None);
    }

    /// Premium is never returned for routing even when a premium model is
    /// configured — the engine must not downgrade a premium turn.
    #[test]
    fn tier_model_never_returns_premium() {
        let c = ProviderCompat {
            tier_models: Some(TierModels {
                cheap: Some("haiku".into()),
                premium: Some("opus".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(c.tier_model("premium"), None);
    }

    /// `[compat.tier_models]` parses from TOML and survives merge (user wins).
    #[test]
    fn tier_models_deserialize_and_merge() {
        let toml_str = r#"
[tier_models]
cheap = "claude-haiku-4"
balanced = "claude-sonnet-4"
"#;
        let user: ProviderCompat = toml::from_str(toml_str).unwrap();
        assert_eq!(user.tier_model("cheap"), Some("claude-haiku-4"));
        assert_eq!(user.tier_model("balanced"), Some("claude-sonnet-4"));

        // Preset has no tier_models; user's map wins through merge.
        let merged = ProviderCompat::merge(ProviderCompat::anthropic_defaults(), user);
        assert_eq!(merged.tier_model("cheap"), Some("claude-haiku-4"));
    }

    #[test]
    fn anthropic_preset_has_cost_rows() {
        let c = ProviderCompat::anthropic_defaults();
        assert!(c.cost_per_input_token.unwrap_or(0.0) > 0.0);
        assert!(c.cost_per_output_token.unwrap_or(0.0) > 0.0);
        assert!(c.cost_per_cache_read_token.unwrap_or(0.0) > 0.0);
        assert!(c.cost_per_cache_write_token.unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn bedrock_preset_has_cost_rows() {
        let c = ProviderCompat::bedrock_defaults();
        assert!(c.cost_per_input_token.unwrap_or(0.0) > 0.0);
        assert!(c.cost_per_output_token.unwrap_or(0.0) > 0.0);
    }

    /// Fix(pricing-audit-2026-05-24): openai_defaults() now uses Some(0.0) as a sentinel.
    /// The old $8/$32 values were a silent 53x overcharge on unrecognised OpenAI models.
    /// Cost attribution is still enabled (Some(0.0) vs None); real pricing resolves via catalog.
    #[test]
    fn openai_preset_has_cost_rows() {
        let c = ProviderCompat::openai_defaults();
        // Some(0.0) sentinel: cost attribution gate fires (is_some() = true)
        // but unmatched models report $0 rather than the stale GPT-5-class rate.
        assert_eq!(c.cost_per_input_token, Some(0.0));
        assert_eq!(c.cost_per_output_token, Some(0.0));
    }

    #[test]
    fn vertex_inherits_anthropic_cost_rows_with_vertex_type() {
        let v = ProviderCompat::vertex_defaults();
        let a = ProviderCompat::anthropic_defaults();
        assert_eq!(v.provider_type(), "vertex");
        assert_eq!(v.cost_per_input_token, a.cost_per_input_token);
        assert_eq!(v.cost_per_output_token, a.cost_per_output_token);
    }

    #[test]
    fn ollama_preset_is_zero_cost() {
        let c = ProviderCompat::ollama_defaults();
        assert_eq!(c.cost_per_input_token, Some(0.0));
        assert_eq!(c.cost_per_output_token, Some(0.0));
    }

    #[test]
    fn unknown_provider_type_when_not_set() {
        let c = ProviderCompat::default();
        assert_eq!(c.provider_type(), "unknown");
    }

    #[test]
    fn merge_user_cost_overrides_default() {
        let defaults = ProviderCompat::anthropic_defaults();
        let user = ProviderCompat {
            cost_per_input_token: Some(0.0), // override to free
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.cost_per_input_token, Some(0.0));
        // Non-overridden cost rows still inherit from defaults.
        assert!(merged.cost_per_output_token.unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn merge_user_provider_type_overrides_default() {
        let defaults = ProviderCompat::anthropic_defaults();
        let user = ProviderCompat {
            provider_type: Some("custom-fork".into()),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.provider_type(), "custom-fork");
    }
}

// --- D.2 (v0.6.3): Tier-2 provider presets report their real id + no
// GPT-class cost rows ---
#[cfg(test)]
mod d2_tier2_provider_cost_tests {
    use super::*;

    /// Each Tier-2 provider preset must report its OWN provider id, NOT
    /// "openai" — otherwise cost attribution mislabels every spend and the
    /// pricing-catalog lookup is wrong-keyed.
    #[test]
    fn tier2_presets_report_their_real_provider_id() {
        assert_eq!(
            ProviderCompat::azure_openai_defaults().provider_type(),
            "azure-openai"
        );
        assert_eq!(
            ProviderCompat::together_defaults().provider_type(),
            "together"
        );
        assert_eq!(
            ProviderCompat::fireworks_defaults().provider_type(),
            "fireworks"
        );
        assert_eq!(ProviderCompat::nvidia_defaults().provider_type(), "nvidia");
        assert_eq!(
            ProviderCompat::perplexity_defaults().provider_type(),
            "perplexity"
        );
        assert_eq!(
            ProviderCompat::cerebras_defaults().provider_type(),
            "cerebras"
        );
    }

    /// Tier-2 presets must NOT carry the inline GPT-class cost rows from
    /// `openai_defaults()`.
    ///
    /// F-026 update: cost rows are now `Some(0.0)` rather than `None`.
    /// `Some(0.0)` is a sentinel meaning "cost attribution is enabled and
    /// events should be emitted, but real pricing resolves via the
    /// `wcore-pricing` catalog; use 0.0 as a floor when the model isn't
    /// in the catalog." This sentinel makes the bootstrap cost-attribution
    /// gate (`bootstrap.rs:1093-1097`) trigger for all openai-compat
    /// secondaries (OpenRouter, Groq, Deepseek, etc.) so `session_cost`
    /// events flow even when exact pricing is catalog-only.
    ///
    /// The important invariant that IS preserved: none of these carry
    /// GPT-class prices ($8/$32 per Mtok). 0.0 is unambiguously not
    /// a GPT-class price.
    #[test]
    fn tier2_presets_have_no_inline_cost_rows() {
        for c in [
            ProviderCompat::azure_openai_defaults(),
            ProviderCompat::together_defaults(),
            ProviderCompat::fireworks_defaults(),
            ProviderCompat::nvidia_defaults(),
            ProviderCompat::perplexity_defaults(),
            ProviderCompat::cerebras_defaults(),
        ] {
            // F-026: cost_per_input_token is now Some(0.0) as a sentinel, not None.
            // Assert that it is NOT the GPT-class price — that's the load-bearing
            // invariant (D.2 v0.6.3 was preventing over-billing, not preventing cost emission).
            assert_ne!(
                c.cost_per_input_token,
                Some(8.0 / 1_000_000.0),
                "provider {} must not carry GPT-class input price",
                c.provider_type()
            );
            assert_ne!(
                c.cost_per_output_token,
                Some(32.0 / 1_000_000.0),
                "provider {} must not carry GPT-class output price",
                c.provider_type()
            );
            // Sentinel value: exactly 0.0 (enables cost attribution gate without
            // fabricating a price; real pricing comes from the catalog).
            assert_eq!(
                c.cost_per_input_token,
                Some(0.0),
                "provider {} must use Some(0.0) sentinel for cost attribution (F-026)",
                c.provider_type()
            );
        }
    }

    /// Tier-2 presets keep the OpenAI *wire-shape* behavioural flags —
    /// only identity and cost change.
    #[test]
    fn tier2_presets_keep_openai_wire_behaviour() {
        let c = ProviderCompat::together_defaults();
        assert!(c.merge_assistant_messages());
        assert!(c.clean_orphan_tool_calls());
        assert!(c.dedup_tool_results());
        assert!(c.supports_effort());
        assert_eq!(c.max_tokens_field.as_deref(), Some("max_tokens"));
    }

    /// OpenAI itself is still labelled "openai" with cost rows present (Some(0.0) sentinel).
    /// Real per-model pricing resolves via the pricing.toml catalog (gpt-4o, gpt-4o-mini, etc.).
    #[test]
    fn openai_preset_unchanged() {
        let c = ProviderCompat::openai_defaults();
        assert_eq!(c.provider_type(), "openai");
        // Some(0.0): cost attribution gate fires; catalog provides real rates.
        assert!(c.cost_per_input_token.is_some());
    }
}

// --- route-gate: input_optimization capability flag ---
//
// `input_optimization` records whether the destination endpoint optimizes
// request input server-side (a router) or expects the client to do it (a
// direct provider). It gates client-side token-optimization passes elsewhere
// in the engine. Vendor-neutral capability — no billing/savings/arbitrage.
#[cfg(test)]
mod input_optimization_tests {
    use super::*;

    /// Native Bash compaction defaults ON (unset ⇒ true) and honours an
    /// explicit `false` override.
    #[test]
    fn compact_bash_defaults_on_and_honors_override() {
        let mut c = ProviderCompat::default();
        assert!(c.compact_bash(), "default must be ON");
        c.compact_bash = Some(false);
        assert!(!c.compact_bash());
    }

    /// Flux Router is a server-side routing layer → "router".
    #[test]
    fn flux_router_preset_is_router() {
        let c = ProviderCompat::flux_router_defaults();
        assert_eq!(c.input_optimization, Some("router".to_string()));
        assert_eq!(c.input_optimization(), "router");
    }

    /// OpenRouter is a genuine server-side router (non-owned vendor) → "router".
    /// A reviewer grepping "router" must find at least two distinct vendors.
    #[test]
    fn openrouter_preset_is_router() {
        let c = ProviderCompat::openrouter_defaults();
        assert_eq!(c.input_optimization, Some("router".to_string()));
        assert_eq!(c.input_optimization(), "router");
    }

    /// Direct providers leave the flag unset → accessor resolves to "client".
    #[test]
    fn direct_providers_are_client() {
        // OpenAI direct.
        let openai = ProviderCompat::openai_defaults();
        assert_eq!(openai.input_optimization, None);
        assert_eq!(openai.input_optimization(), "client");

        // Anthropic direct.
        let anthropic = ProviderCompat::anthropic_defaults();
        assert_eq!(anthropic.input_optimization, None);
        assert_eq!(anthropic.input_optimization(), "client");
    }

    /// Plain OpenAI-compat *providers* (not routers) stay "client" even though
    /// they share the `openai_compat_provider()` constructor with the routers.
    #[test]
    fn openai_compat_non_routers_are_client() {
        for c in [
            ProviderCompat::together_defaults(),
            ProviderCompat::groq_defaults(),
            ProviderCompat::deepseek_defaults(),
        ] {
            assert_eq!(
                c.input_optimization,
                None,
                "provider {} is a direct provider, not a router",
                c.provider_type()
            );
            assert_eq!(c.input_optimization(), "client");
        }
    }

    /// The accessor defaults to "client" when the flag is entirely unset.
    #[test]
    fn accessor_defaults_to_client_when_none() {
        let c = ProviderCompat::default();
        assert_eq!(c.input_optimization, None);
        assert_eq!(c.input_optimization(), "client");
    }

    /// A user-set `Some` wins over the preset default through `merge()`.
    #[test]
    fn merge_user_input_optimization_overrides_default() {
        // User forces "router" on a direct provider that defaults to None.
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            input_optimization: Some("router".to_string()),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.input_optimization, Some("router".to_string()));
        assert_eq!(merged.input_optimization(), "router");
    }

    /// An empty user keeps the router default (here: a router preset).
    #[test]
    fn merge_empty_user_keeps_router_default() {
        let defaults = ProviderCompat::flux_router_defaults();
        let merged = ProviderCompat::merge(defaults, ProviderCompat::default());
        assert_eq!(merged.input_optimization(), "router");
    }

    /// Crucible #3 — merge ripple: a user `supports_temperature = false` MUST
    /// survive `merge()` over a preset that defaults it `Some(true)` (the
    /// reference_providercompat_merge_ripple gotcha). An empty user keeps the
    /// preset's `Some(true)`; an unset field resolves to `true` via the accessor.
    #[test]
    fn merge_user_supports_temperature_overrides_default() {
        let defaults = ProviderCompat::openai_defaults();
        assert_eq!(defaults.supports_temperature, Some(true));

        // User opts a quirky endpoint OUT of temperature.
        let user = ProviderCompat {
            supports_temperature: Some(false),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(defaults.clone(), user);
        assert_eq!(merged.supports_temperature, Some(false));
        assert!(!merged.supports_temperature());

        // Empty user keeps the preset's Some(true).
        let merged_empty = ProviderCompat::merge(defaults, ProviderCompat::default());
        assert_eq!(merged_empty.supports_temperature, Some(true));
        assert!(merged_empty.supports_temperature());

        // Unset everywhere → accessor defaults to true.
        assert!(ProviderCompat::default().supports_temperature());
    }

    /// #112 — the omit-safe presets carry `omit_max_tokens_when_unsized =
    /// Some(true)`; every send-a-sized-value provider stays off it.
    #[test]
    fn omit_max_tokens_when_unsized_preset_coverage() {
        // Omit-safe: the endpoint defaults the field per served model.
        assert!(ProviderCompat::gemini_defaults().omit_max_tokens_when_unsized());
        assert!(ProviderCompat::openrouter_defaults().omit_max_tokens_when_unsized());
        assert!(ProviderCompat::flux_router_defaults().omit_max_tokens_when_unsized());

        // Never omit: anthropic's Messages API mandates `max_tokens`; native
        // openai + generic openai-compat endpoints keep a sized value (vLLM
        // et al. may 400 without the field or default to a tiny ceiling).
        assert!(!ProviderCompat::anthropic_defaults().omit_max_tokens_when_unsized());
        assert!(!ProviderCompat::openai_defaults().omit_max_tokens_when_unsized());
        assert!(!ProviderCompat::together_defaults().omit_max_tokens_when_unsized());
        assert!(!ProviderCompat::default().omit_max_tokens_when_unsized());
    }

    /// #112 — merge ripple (the reference_providercompat_merge_ripple gotcha):
    /// a user override MUST survive `merge()` in both directions — opting a
    /// quirky endpoint IN over a default-off preset, and opting OUT over an
    /// omit-safe preset. An empty user keeps the preset value.
    #[test]
    fn merge_user_omit_max_tokens_when_unsized_overrides_default() {
        // User opts a custom endpoint IN.
        let user_on = ProviderCompat {
            omit_max_tokens_when_unsized: Some(true),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(ProviderCompat::openai_defaults(), user_on);
        assert_eq!(merged.omit_max_tokens_when_unsized, Some(true));
        assert!(merged.omit_max_tokens_when_unsized());

        // User opts an omit-safe router OUT.
        let user_off = ProviderCompat {
            omit_max_tokens_when_unsized: Some(false),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(ProviderCompat::flux_router_defaults(), user_off);
        assert_eq!(merged.omit_max_tokens_when_unsized, Some(false));
        assert!(!merged.omit_max_tokens_when_unsized());

        // Empty user keeps the omit-safe preset default.
        let merged_empty = ProviderCompat::merge(
            ProviderCompat::flux_router_defaults(),
            ProviderCompat::default(),
        );
        assert_eq!(merged_empty.omit_max_tokens_when_unsized, Some(true));
    }

    #[test]
    fn failover_capability_presets_match_native_wire_support() {
        let anthropic = ProviderCompat::anthropic_defaults();
        assert!(anthropic.supports_tools());
        assert!(anthropic.supports_vision());
        assert!(!anthropic.supports_structured_output());

        let gemini = ProviderCompat::gemini_defaults();
        assert!(gemini.supports_tools());
        assert!(gemini.supports_vision());

        let openai = ProviderCompat::openai_defaults();
        assert!(openai.supports_tools());
        assert!(openai.supports_vision());
        assert!(openai.supports_structured_output());

        let unknown = ProviderCompat::default();
        assert!(!unknown.supports_tools());
        assert!(!unknown.supports_vision());
        assert!(!unknown.supports_structured_output());
    }

    // -- F-27C3-04: per-provider image model ---------------------------

    /// The two providers that actually serve `/v1/images/generations` must
    /// declare DIFFERENT model ids. Asserting each value alone would still
    /// pass if both were the same constant, which is precisely the defect:
    /// one global `gpt-image-1` sent to a FluxRouter key that is not entitled
    /// to it, so the built-in `image_generate` tool failed by default for
    /// every FluxRouter user (measured live by lane `27-c3-media`).
    #[test]
    fn image_model_differs_between_openai_and_flux_router() {
        let openai = ProviderCompat::openai_defaults().image_model;
        let flux = ProviderCompat::flux_router_defaults().image_model;
        assert_eq!(openai.as_deref(), Some("gpt-image-1"));
        assert_eq!(flux.as_deref(), Some("flux-image"));
        assert_ne!(
            openai, flux,
            "the image model must be provider-specific; equal values mean the \
             hardcoded global default is back"
        );
    }

    /// An openai-compat secondary is NOT OpenAI, so it must not inherit
    /// OpenAI's image model — the same reason the cost rows are cleared.
    #[test]
    fn openai_compat_secondaries_do_not_inherit_openai_image_model() {
        assert_eq!(ProviderCompat::together_defaults().image_model, None);
        assert_eq!(ProviderCompat::groq_defaults().image_model, None);
        assert_eq!(ProviderCompat::azure_openai_defaults().image_model, None);
        // Control: the inheritance path IS live — these same presets DO pick
        // up an OpenAI behavioural flag. Without this the assertions above
        // would pass equally on a preset chain that inherited nothing at all.
        assert_eq!(
            ProviderCompat::together_defaults()
                .max_tokens_field
                .as_deref(),
            Some("max_tokens"),
            "control: together must still inherit openai_defaults' wire shape"
        );
        // And a provider outside the OpenAI-wire family never had one.
        assert_eq!(ProviderCompat::anthropic_defaults().image_model, None);
    }

    /// The `merge()` ripple. A field that is not threaded through `merge()`
    /// compiles fine and silently discards every user override — the gotcha
    /// the Crucible #3 comment in `merge()` records. This proves both
    /// directions: the user wins when set, the preset survives when not.
    #[test]
    fn merge_user_image_model_overrides_preset() {
        let user = ProviderCompat {
            image_model: Some("dall-e-3".into()),
            ..ProviderCompat::default()
        };
        let merged = ProviderCompat::merge(ProviderCompat::flux_router_defaults(), user);
        assert_eq!(merged.image_model.as_deref(), Some("dall-e-3"));

        let merged_empty = ProviderCompat::merge(
            ProviderCompat::flux_router_defaults(),
            ProviderCompat::default(),
        );
        assert_eq!(merged_empty.image_model.as_deref(), Some("flux-image"));
    }

    /// `[compat] image_model = "..."` must survive TOML deserialization —
    /// the field is useless as an operator override otherwise.
    #[test]
    fn image_model_round_trips_through_toml() {
        let parsed: ProviderCompat =
            toml::from_str(r#"image_model = "gpt-image-1-mini""#).expect("compat toml parses");
        assert_eq!(parsed.image_model.as_deref(), Some("gpt-image-1-mini"));
        // Control: an absent key stays None rather than defaulting to a value.
        let empty: ProviderCompat = toml::from_str("").expect("empty compat toml parses");
        assert_eq!(empty.image_model, None);
    }
}

#[cfg(test)]
mod cost_rate_provenance_tests {
    use super::{CostRateProvenance, ProviderCompat};

    /// A user who configures nothing inherits the whole preset row, and none
    /// of it is theirs. `anthropic_defaults()` carries the Opus list price
    /// for every Anthropic model, so treating it as authoritative is how a
    /// $90 estimate got reported as an $18 model's spend.
    #[test]
    fn preset_rates_are_not_user_supplied() {
        let merged = ProviderCompat::merge(
            ProviderCompat::anthropic_defaults(),
            ProviderCompat::default(),
        );
        assert!(merged.cost_per_input_token.unwrap_or(0.0) > 0.0, "control");
        assert_eq!(merged.cost_rate_provenance, CostRateProvenance::default());
    }

    /// The user overrides two axes; those two become theirs and the two they
    /// left alone stay the preset's. Per-axis, not per-profile — a profile
    /// verdict would either forfeit the user's rates or launder the preset's.
    #[test]
    fn merge_records_provenance_per_axis() {
        let merged = ProviderCompat::merge(
            ProviderCompat::anthropic_defaults(),
            ProviderCompat {
                cost_per_input_token: Some(0.000_003),
                cost_per_output_token: Some(0.000_015),
                ..Default::default()
            },
        );
        assert_eq!(
            merged.cost_rate_provenance,
            CostRateProvenance {
                input: true,
                output: true,
                cache_read: false,
                cache_write: false,
            }
        );
    }

    /// An axis nobody priced cannot taint the profile: there is no fabricated
    /// number on it to mistake for a price.
    #[test]
    fn an_absent_axis_is_not_a_fabrication() {
        let merged = ProviderCompat::merge(
            ProviderCompat::cohere_defaults(),
            ProviderCompat {
                cost_per_input_token: Some(0.000_003),
                ..Default::default()
            },
        );
        assert_eq!(
            merged.cost_rate_provenance,
            CostRateProvenance {
                input: true,
                output: true,
                cache_read: true,
                cache_write: true,
            },
            "cohere_defaults() carries no rates, so nothing was inherited"
        );
    }

    /// Provenance is EARNED by carrying the rate, never ASSERTED. A config
    /// file that names the field must not be able to certify a preset row —
    /// and a serialization round-trip must land on the fail-safe side.
    #[test]
    fn provenance_cannot_be_asserted_by_a_config_file() {
        let user: ProviderCompat = toml::from_str(
            "cost_rate_provenance = { input = true, output = true, \
             cache_read = true, cache_write = true }\n",
        )
        .expect("unknown compat keys are ignored, not rejected");
        assert_eq!(user.cost_rate_provenance, CostRateProvenance::default());

        let merged = ProviderCompat::merge(ProviderCompat::anthropic_defaults(), user);
        assert_eq!(
            merged.cost_rate_provenance,
            CostRateProvenance::default(),
            "the preset row stays the preset's"
        );

        let authoritative = ProviderCompat::merge(
            ProviderCompat::anthropic_defaults(),
            ProviderCompat {
                cost_per_input_token: Some(0.000_003),
                ..Default::default()
            },
        );
        assert!(
            authoritative.cost_rate_provenance.input,
            "control: this profile IS authoritative on the input axis"
        );
        let round_tripped: ProviderCompat =
            serde_json::from_str(&serde_json::to_string(&authoritative).expect("serialize"))
                .expect("deserialize");
        assert_eq!(
            round_tripped.cost_rate_provenance,
            CostRateProvenance::default(),
            "a round-trip must forget authority, never invent it"
        );
    }
}

// --- #559 Layer E1: prompt_cache_expected must not inherit ---

#[cfg(test)]
mod prompt_cache_expected_pinning_tests {
    use super::*;
    use crate::config::{ProviderType, compat_defaults_for};

    /// Every `ProviderType`, so the table below is read off the REAL preset
    /// constructors (via `compat_defaults_for`) rather than a hand-built
    /// `ProviderCompat`. A hand-built struct literal cannot observe
    /// struct-update inheritance — which is exactly how `prompt_cache_expected`
    /// leaked from `openai_defaults()` into every openai-compat preset and from
    /// `anthropic_defaults()` into Vertex and MiniMax.
    const ALL_PROVIDER_TYPES: [ProviderType; 23] = [
        ProviderType::Anthropic,
        ProviderType::OpenAI,
        ProviderType::Bedrock,
        ProviderType::Vertex,
        ProviderType::Gemini,
        ProviderType::AzureOpenAI,
        ProviderType::Together,
        ProviderType::Fireworks,
        ProviderType::Nvidia,
        ProviderType::Perplexity,
        ProviderType::Cerebras,
        ProviderType::OpenRouter,
        ProviderType::FluxRouter,
        ProviderType::Deepseek,
        ProviderType::Xai,
        ProviderType::Groq,
        ProviderType::Moonshot,
        ProviderType::Qwen,
        ProviderType::Mistral,
        ProviderType::Cohere,
        ProviderType::OpenAIChatGpt,
        ProviderType::MiniMax,
        ProviderType::Sakana,
    ];

    /// The ONLY presets allowed to answer `true`.
    ///
    /// `prompt_cache_expected` is an accusation: `true` makes the engine tell
    /// the user their provider is failing to serve prompt cache and re-billing
    /// them. It may only be `true` where the preset carries an in-line
    /// justification for the claim — today Anthropic and Bedrock (explicit
    /// `cache_control` + priced cache reads), native OpenAI (automatic caching
    /// reported as `prompt_tokens_details.cached_tokens`) and FluxRouter
    /// (measured live, see `flux_router_defaults`).
    ///
    /// This `match` is exhaustive on purpose: a new `ProviderType` will not
    /// compile until someone classifies it, and the default classification for
    /// anything unmeasured is `false` — "unknown, never accuse".
    fn justified_to_expect_a_prompt_cache(provider: ProviderType) -> bool {
        match provider {
            ProviderType::Anthropic
            | ProviderType::Bedrock
            | ProviderType::OpenAI
            | ProviderType::FluxRouter => true,
            ProviderType::Vertex
            | ProviderType::Gemini
            | ProviderType::AzureOpenAI
            | ProviderType::Together
            | ProviderType::Fireworks
            | ProviderType::Nvidia
            | ProviderType::Perplexity
            | ProviderType::Cerebras
            | ProviderType::OpenRouter
            | ProviderType::Deepseek
            | ProviderType::Xai
            | ProviderType::Groq
            | ProviderType::Moonshot
            | ProviderType::Qwen
            | ProviderType::Mistral
            | ProviderType::Cohere
            | ProviderType::OpenAIChatGpt
            | ProviderType::MiniMax
            | ProviderType::Sakana => false,
        }
    }

    #[test]
    fn only_justified_presets_expect_a_served_prompt_cache() {
        assert_eq!(
            ALL_PROVIDER_TYPES.len(),
            23,
            "ALL_PROVIDER_TYPES is stale — append the new ProviderType variant \
             (the match in justified_to_expect_a_prompt_cache already forced you \
             to classify it)"
        );

        let mut wrong: Vec<String> = Vec::new();
        println!("provider                 resolved  justified");
        for provider in ALL_PROVIDER_TYPES {
            let resolved = compat_defaults_for(provider).prompt_cache_expected();
            let justified = justified_to_expect_a_prompt_cache(provider);
            let name = format!("{provider:?}");
            println!("{name:<24} {resolved:<9} {justified}");
            if resolved != justified {
                wrong.push(format!(
                    "{provider:?}: resolved={resolved} justified={justified}"
                ));
            }
        }
        // Known-negative controls, in the same run: neither is reachable via
        // `ProviderType`, and neither may ever be accused.
        let ollama = ProviderCompat::ollama_defaults().prompt_cache_expected();
        let bare = ProviderCompat::default().prompt_cache_expected();
        println!("{:<24} {:<9} {}", "ollama_defaults()", ollama, false);
        println!("{:<24} {:<9} {}", "ProviderCompat::default()", bare, false);
        assert!(!ollama, "control: ollama must never expect a prompt cache");
        assert!(
            !bare,
            "control: a bare ProviderCompat must never expect one"
        );

        assert!(
            wrong.is_empty(),
            "prompt_cache_expected leaked into presets with no justification for \
             the claim (struct-update inheritance from openai_defaults() / \
             anthropic_defaults()). Each of these would tell the user their \
             provider is silently re-billing an uncached prompt, with zero \
             evidence for that route:\n  {}",
            wrong.join("\n  ")
        );
    }

    #[test]
    fn openai_compat_provider_does_not_inherit_the_openai_cache_claim() {
        // The direct unit of the leak: the shared Tier-2 constructor. Native
        // OpenAI keeps the claim; a Tier-2 provider that merely speaks the
        // OpenAI wire does not inherit it.
        assert_eq!(
            ProviderCompat::openai_defaults().prompt_cache_expected,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::openai_compat_provider("groq").prompt_cache_expected,
            None,
            "openai_compat_provider() must clear the claim, not inherit it"
        );
        assert!(!ProviderCompat::openai_compat_provider("groq").prompt_cache_expected());
    }

    #[test]
    fn anthropic_shaped_presets_do_not_inherit_the_anthropic_cache_claim() {
        assert_eq!(
            ProviderCompat::anthropic_defaults().prompt_cache_expected,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::vertex_defaults().prompt_cache_expected,
            None
        );
        assert_eq!(
            ProviderCompat::minimax_defaults().prompt_cache_expected,
            None
        );
        assert!(!ProviderCompat::vertex_defaults().prompt_cache_expected());
        assert!(!ProviderCompat::minimax_defaults().prompt_cache_expected());
    }

    #[test]
    fn a_user_can_still_opt_a_tier2_provider_in() {
        // Clearing the preset must not remove the user's ability to declare the
        // capability for their own endpoint.
        let merged = ProviderCompat::merge(
            ProviderCompat::groq_defaults(),
            ProviderCompat {
                prompt_cache_expected: Some(true),
                ..ProviderCompat::default()
            },
        );
        assert!(merged.prompt_cache_expected());
    }
}
