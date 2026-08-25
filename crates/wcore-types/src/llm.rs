use serde_json::Value;

use crate::cache_tier::CacheTier;
use crate::message::{FinishReason, StopReason, TokenUsage, ToolUseId};
use crate::tool::ToolDef;

/// W1 v0.6.3: free-form routing label attached to an `LlmRequest`.
///
/// Defined as a newtype here (in `wcore-types`) — NOT in `wcore-providers` —
/// because `LlmRequest` lives in `wcore-types` and `wcore-providers` already
/// depends on `wcore-types`. Putting the hint type in `wcore-providers` would
/// reintroduce the exact circular-dep that the W8 `CacheTier` move just broke
/// (`wcore-types::llm` referencing a `wcore-providers` type).
///
/// The richer `RequestShape` / `RoutingDecision` types in
/// `wcore-providers::routing` are the *producers*: they map a shape to a
/// stable string label which gets stamped onto the request here. Providers
/// downstream of the router consult this hint opportunistically; unknown
/// labels are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingHint(pub String);

impl RoutingHint {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// The canonical `loop_owner` value wayland-core sends: this process runs a
/// CLIENT-side Anvil climb.
pub const ANVIL_LOOP_OWNER: &str = "anvil";

/// #863 F2/F5 — loop-ownership provenance for the Flux anti-collision
/// handshake, carried on [`LlmRequest::flux_loop_intent`].
///
/// The contract has exactly one rule: **one ladder per task**. wayland-core's
/// Anvil is a client-side climb; Flux's Elevation is a server-side climb; a
/// request must never run both. The two arms below are the only two things a
/// caller can say about that, and they are mutually exclusive **by
/// construction** — there is no representable state in which a turn both
/// declares Core owns the loop and opts into Flux running its own. That is F5
/// ("Core must never set `flux_verify` implicitly on driver traffic") enforced
/// by the type system rather than by a guard somebody can forget to call.
///
/// NOTE the naming. The wire field is `loop_owner`, but nothing in Rust here is
/// called `loop_owner`: the Goals subsystem (`wcore_agent::goal`) has its own,
/// entirely unrelated `loop_owner` concept spread over ~14 files. The `flux_`
/// prefix keeps the two greppable apart — a plain `loop_owner` grep in this
/// workspace returns Goals hits and nothing to do with Flux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FluxLoopIntent {
    /// Core owns the loop for this turn — mid-loop material of a client-side
    /// climb. Flux MUST NOT engage Elevation on it regardless of alias, and
    /// MUST NOT stamp verification on it. Emitted as `X-Flux-Loop-Owner` (and
    /// `metadata.loop_owner` on OpenAI-wire bodies).
    ClientOwned(String),
    /// Explicit per-request opt-in to Flux running its OWN server-side
    /// Elevation ladder on `flux-auto` (Flux's C2 decision). Emitted as
    /// `X-Flux-Verify` (and `metadata.flux_verify` on OpenAI-wire bodies). Core
    /// never sets this implicitly — it exists so a caller who genuinely wants
    /// the server ladder can ask for it, and asking for it makes `ClientOwned`
    /// unrepresentable on the same turn.
    ServerVerify,
}

impl FluxLoopIntent {
    /// The `loop_owner` value to put on the wire, or `None` for `ServerVerify`.
    pub fn owner(&self) -> Option<&str> {
        match self {
            Self::ClientOwned(owner) => Some(owner.as_str()),
            Self::ServerVerify => None,
        }
    }

    /// Whether this turn opts into Flux's server-side Elevation ladder.
    pub fn is_server_verify(&self) -> bool {
        matches!(self, Self::ServerVerify)
    }
}

/// A request to the LLM provider
///
/// W8 v0.6.3: `cache_tier` lets callers express an Anthropic prompt-cache
/// preference; consumed by `apply_cache_zones`. W1 v0.6.3: `routing_hint`
/// carries a stable label from the smart router for ProviderChain dispatch.
///
/// `Default` is derived so new fields can be added by callers via
/// `..Default::default()` without breaking the 45+ existing struct-literal
/// construction sites; the v0.6.3 sweep adds the two new fields explicitly at
/// every site for greppability.
#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<crate::message::Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    /// Optional: thinking config (Anthropic extended thinking)
    pub thinking: Option<ThinkingConfig>,
    /// Optional: reasoning effort for OpenAI reasoning models (low/medium/high)
    pub reasoning_effort: Option<String>,
    /// W8 v0.6.3: prompt-cache tier picked upstream by `pick_cache_tier`.
    /// `None` means the provider falls back to its built-in heuristic
    /// (currently hard-coded `Ephemeral5m` for Anthropic).
    pub cache_tier: Option<CacheTier>,
    /// W1 v0.6.3: smart-routing hint produced by `wcore-providers::routing`
    /// and consumed by `ProviderChain`. Free-form label — providers ignore
    /// hints they don't recognize.
    pub routing_hint: Option<RoutingHint>,
    /// Output-side token optimization: extra stop sequences that providers
    /// UNION (never replace) into their native stop-sequence field, so the
    /// model halts the moment it begins a known fluff closer at a paragraph
    /// boundary. Populated by the engine ONLY when the route optimizes
    /// client-side (`compat.input_optimization() == "client"`); empty for
    /// router-optimized routes. `Default` yields an empty Vec, so all
    /// existing `..Default::default()` construction sites stay back-compatible
    /// and emit no stop field.
    pub stop_sequences: Vec<String>,
    /// FluxRouter web_search grounding (contract §5). When `true`, the OpenAI
    /// provider attaches a `{"type":"web_search"}` tool to the chat request so
    /// Flux server-side-grounds the turn via Perplexity Sonar and streams back
    /// `citations` / `search_results`. Grounding only fires on a **tier-alias**
    /// model (`flux-auto` / `flux-fast` / `flux-standard` / `flux-reasoning`);
    /// the provider skips injection for a concrete model id. `Default` is
    /// `false`, so all existing construction sites stay ungrounded.
    pub web_search: bool,
    /// #282 contract V1: stable, per-session conversation id for Flux sticky
    /// routing. Emitted as the `x-wl-conversation-id` request header ONLY on a
    /// Flux tier-alias request; `None` (the default) skips the header, so all
    /// existing `..Default::default()` construction sites stay back-compatible.
    /// Minted once at engine construction with a v4 UUID and threaded onto every
    /// request the engine builds.
    pub conversation_id: Option<String>,
    /// #282 contract V1: the full assembled-prompt token estimate for this turn
    /// (system + tools + messages), as computed by the engine before the stream
    /// call. Emitted as the `x-wl-context-tokens` request header ONLY on a Flux
    /// tier-alias request; `None` (the default) skips the header. Kept as an
    /// `Option` so providers/tests that don't supply an estimate stay
    /// back-compatible via `..Default::default()`.
    pub client_context_tokens: Option<u64>,
    /// Crucible #3: optional sampling temperature. `None` (the default) means
    /// the provider uses its own default and omits the field entirely.
    /// Providers that reject an explicit temperature (OpenAI `o1*`/`o3*`
    /// reasoning families) drop it via `openai_compat::accepts_temperature`;
    /// a provider can also opt out via `ProviderCompat.supports_temperature`.
    /// `Default` is `None`, so all existing `..Default::default()` construction
    /// sites stay back-compatible.
    pub temperature: Option<f32>,
    /// #112 — when `true`, providers that tolerate an absent output cap OMIT
    /// their max-tokens wire field (`max_tokens` / `max_completion_tokens` /
    /// `max_output_tokens` / `generationConfig.maxOutputTokens`) so the served
    /// model's natural ceiling applies. Set by the engine ONLY when ALL hold:
    /// the model is unknown to `wcore_config::limits`, the user omitted
    /// `--max-tokens` (no CLI flag, no non-default TOML), and the provider's
    /// `ProviderCompat.omit_max_tokens_when_unsized` is on. `max_tokens` above
    /// STAYS at the sized positive internal budget regardless (it still feeds
    /// `fit_thinking_budget`, the `x-wl-expected-output` header, and the #255
    /// gauge math) — this flag governs the WIRE field only. Anthropic ignores
    /// it (the Messages API mandates `max_tokens`). `Default` is `false`, so
    /// all existing `..Default::default()` construction sites keep sending the
    /// field.
    pub omit_max_tokens: bool,
    /// #863 F2/F5 — loop-ownership provenance for the Flux anti-collision
    /// contract. `Some(ClientOwned("anvil"))` marks this turn as mid-loop
    /// material of wayland-core's CLIENT-side Anvil climb, so Flux must not run
    /// its server-side Elevation ladder on it; `Some(ServerVerify)` is the
    /// opposite, explicit opt-in. See [`FluxLoopIntent`] for why the two cannot
    /// coexist and why nothing here is spelled `loop_owner`.
    ///
    /// Whether this reaches the wire is decided by the ENDPOINT, not by this
    /// field and not by the model name: providers emit it only when
    /// `ProviderCompat::flux_loop_provenance()` is on. That is deliberate — the
    /// F2 contract says Flux honours `loop_owner` "regardless of alias", so
    /// gating emission on a tier-alias name (the way the #282 `x-wl-*` headers
    /// do) would silently drop the marking on a concrete-model driver turn,
    /// which is exactly the collision this contract exists to prevent.
    ///
    /// `Default` is `None`, so every existing `..Default::default()`
    /// construction site stays back-compatible and emits nothing.
    pub flux_loop_intent: Option<FluxLoopIntent>,
    /// #863 F3 — per-turn cache-variance nonce, emitted as `metadata.nonce` on
    /// OpenAI-wire bodies alongside the loop-ownership marking.
    ///
    /// FerroxLabs/wayland#862 measured identical requests returning identical
    /// cached completion ids, which intermittently wedges an iterative client
    /// loop: the loop asks again, gets its own previous answer back, and cannot
    /// make progress. Flux bypasses/varies its semantic cache for requests
    /// carrying `loop_owner`, so this is belt-and-braces — but it is the half
    /// that does not depend on the server keeping its promise.
    ///
    /// Must be DERIVED, never randomly minted at the provider: the session
    /// journal digests the prepared request, so a value that changed between
    /// building a turn and replaying it would break recovery. The engine
    /// derives it from the stable conversation id plus the turn index.
    ///
    /// `Default` is `None`, so all existing construction sites emit nothing.
    pub flux_turn_nonce: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ThinkingConfig {
    Enabled { budget_tokens: u32 },
    Disabled,
}

/// Streaming events from the LLM
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Incremental text output
    TextDelta(String),
    /// Complete tool call (after accumulating streaming deltas)
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
        /// Opaque provider metadata (e.g. Gemini thought_signature) to round-trip.
        extra: Option<Value>,
    },
    /// Thinking content (Anthropic only)
    ThinkingDelta(String),
    /// Per-turn thinking SUBJECT: a short opaque display label for the
    /// reasoning block (e.g. Flux's `delta.reasoning_summary`, a gerund
    /// phrase like "Reasoning through the problem"). Distinct from
    /// `ThinkingDelta` (the raw thinking text). Emitted once per turn,
    /// immediately before the first `ThinkingDelta`, only on turns that
    /// actually produce reasoning. The host renders it as the heading for
    /// the in-flight thinking block. Opaque — never switch on the value.
    ThinkingSubject(String),
    /// C-4b — an opaque provider signature covering the reasoning of THIS
    /// turn, carried on the thought part itself (Gemini `thoughtSignature`).
    /// Gemini is stateless about reasoning: a signed thought must be sent
    /// back verbatim, signature included, or the server rejects the replayed
    /// turn. The signature on a `functionCall` part is a DIFFERENT value and
    /// already rides on [`LlmEvent::ToolUse::extra`]; this variant carries the
    /// one that arrives on a thought part, which had nowhere to go before.
    /// The engine folds it into `ContentBlock::Thinking.extra` for replay.
    /// Emitted at most once per turn (first signature wins); providers that
    /// don't sign reasoning never emit it.
    ThinkingSignature(String),
    /// Response complete
    Done {
        stop_reason: StopReason,
        /// Protocol-level finish reason mapped from the provider's native
        /// stop signal. Populated by each provider; `Error` if the raw
        /// value couldn't be classified (the provider should also log a
        /// warning in that case).
        finish_reason: FinishReason,
        usage: TokenUsage,
    },
    /// The provider stopped at its OUTPUT token cap (`finish_reason=length`)
    /// while a tool call was still streaming, so the accumulated argument JSON
    /// is an unterminated fragment. The call CANNOT be run — half its
    /// arguments never arrived — but dropping it silently is what made a turn
    /// cut off mid-deliverable indistinguishable from a model that had simply
    /// finished talking: the engine saw an empty tool-call list and took the
    /// natural-completion path. Providers emit one of these per pending call
    /// so the engine has a condition to act on instead of a hole.
    ///
    /// NOT an [`LlmEvent::Error`]: the stream itself is well-formed, and the
    /// right response to a severed output differs from a transport fault.
    TruncatedToolCall {
        /// Tool name accumulated before the cut (empty when the cut landed
        /// before the name arrived).
        name: String,
        /// Bytes of argument JSON that did arrive. Sizes the loss for the user
        /// without echoing a half-written payload back at them.
        partial_arg_bytes: usize,
    },
    /// The provider accepted the request and the stream has produced NO
    /// bytes for `silent_for`.
    ///
    /// NOT an error and NOT terminal: the request is still live, the
    /// between-bytes read timeout has not fired, and the turn may still
    /// complete normally. A long silence before the first byte is legitimate
    /// on reasoning models, which is exactly why the read timeout is five
    /// minutes — but from outside, a healthy silent stream and a hung one are
    /// indistinguishable, and the product said nothing for the whole window.
    ///
    /// Carries the elapsed silence and no prose: rendering belongs to the
    /// agent layer, which owns the user's surface. Emitted at most once per
    /// silent gap, and cancelled rather than deferred when the wait ends, by
    /// the two adjacent windows that together cover a whole request:
    /// `wcore_providers::http_client::awaiting_first_byte` from dispatch to the
    /// response head, and `..::next_or_consumer_closed`, which every provider
    /// polls, from there to the first byte and every later gap.
    StreamSilent { silent_for: std::time::Duration },
    /// Error from the API
    Error(String),
    /// FluxRouter web_search grounding (contract §5.4): the deduplicated set of
    /// citation URL strings accumulated across the streamed Sonar frames, index-
    /// aligned with the inline `[1]`/`[2]` markers in the answer text. Emitted
    /// once at end-of-stream when grounding fired (empty otherwise → not sent).
    Citations(Vec<String>),
    /// FluxRouter web_search grounding (contract §5.4): the richer per-source
    /// cards accompanying [`LlmEvent::Citations`]. Emitted once at end-of-stream.
    SearchResults(Vec<FluxSearchResult>),
    /// #282 contract V1 — Flux SIGNALS-BACK response metadata, parsed from the
    /// `x-flux-*` response headers and emitted ONCE at stream start (before any
    /// text deltas). Every field is `Option` because a non-Flux provider never
    /// sends these headers, so a missing/unparsable header is `None` rather than
    /// a stream error. Consumed by the engine to reconcile the #255 context
    /// gauge against the REAL served-model window and to stash live context
    /// pressure for future scheduling (#280).
    ProviderMeta {
        /// `x-flux-routed-model` — the upstream model Flux actually routed to.
        routed_model: Option<String>,
        /// `x-flux-model-window` — the routed model's context window (tokens).
        model_window: Option<u64>,
        /// `x-flux-context-pressure` — `0.0..=1.0` = required / window.
        context_pressure: Option<f32>,
        /// `x-flux-context-tokens-counted` — Flux's own count of the prompt.
        tokens_counted: Option<u64>,
        /// #863 F2 — `x-flux-loop-engaged`: which ladder Flux ran for this
        /// turn (`none` | `cascade` | `elevation`). This is the RUNTIME half of
        /// the anti-collision detector: `elevation` on a turn Core marked
        /// `FluxLoopIntent::ClientOwned` means both ladders ran, and the
        /// candidate it produced is contaminated mid-loop material.
        loop_engaged: Option<String>,
    },
}

/// A single FluxRouter / Perplexity-Sonar web_search source card (contract
/// §5.4). `date` / `last_updated` are frequently absent on a given result, so
/// they deserialize as `None` rather than failing the whole array. `title`,
/// `url`, `snippet`, and `source` default to empty strings when a result omits
/// them (defensive — the live streamed shape is not yet captured).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FluxSearchResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{FinishReason, StopReason, TokenUsage};
    use serde_json::json;

    #[test]
    fn test_thinking_config_enabled_stores_budget() {
        let config = ThinkingConfig::Enabled {
            budget_tokens: 4096,
        };
        match config {
            ThinkingConfig::Enabled { budget_tokens } => assert_eq!(budget_tokens, 4096),
            ThinkingConfig::Disabled => panic!("expected Enabled"),
        }
    }

    #[test]
    fn test_llm_event_text_delta_carries_content() {
        let event = LlmEvent::TextDelta("hello".to_string());
        match event {
            LlmEvent::TextDelta(text) => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_llm_event_done_carries_stop_reason_and_usage() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 5,
        };
        let event = LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage,
        };
        match event {
            LlmEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(finish_reason, FinishReason::Stop);
                assert_eq!(usage.input_tokens, 10);
            }
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn test_finish_reason_from_stop_reason() {
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::EndTurn),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::ToolUse),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::MaxTokens),
            FinishReason::Length
        );
        // #457: the per-turn cap now maps to its own FinishReason::MaxTurns
        // (was Error) so hosts can offer Continue instead of a model-failure UX.
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::MaxTurns),
            FinishReason::MaxTurns
        );
    }

    #[test]
    fn llm_request_default_has_no_cache_tier_or_routing_hint() {
        let req = LlmRequest::default();
        assert!(req.cache_tier.is_none());
        assert!(req.routing_hint.is_none());
        assert!(req.model.is_empty());
        assert!(req.system.is_empty());
        assert_eq!(req.max_tokens, 0);
    }

    #[test]
    fn llm_request_with_cache_tier_round_trips() {
        let req = LlmRequest {
            cache_tier: Some(CacheTier::Ephemeral1h),
            routing_hint: Some(RoutingHint::new("fast")),
            ..Default::default()
        };
        assert!(matches!(req.cache_tier, Some(CacheTier::Ephemeral1h)));
        assert_eq!(req.routing_hint.as_ref().unwrap().0, "fast");
    }

    #[test]
    fn routing_hint_newtype_eq() {
        assert_eq!(RoutingHint::new("a"), RoutingHint("a".to_string()));
        assert_ne!(RoutingHint::new("a"), RoutingHint::new("b"));
    }

    /// Output-side opt (Part A) back-compat: a default-constructed request
    /// carries NO stop sequences, so existing `..Default::default()` callers
    /// keep emitting no provider stop field.
    #[test]
    fn llm_request_default_has_empty_stop_sequences() {
        let req = LlmRequest::default();
        assert!(req.stop_sequences.is_empty());
    }

    #[test]
    fn llm_request_default_has_web_search_false() {
        let req = LlmRequest::default();
        assert!(!req.web_search);
    }

    /// #112 back-compat: a default-constructed request does NOT omit the wire
    /// max-tokens field, so every existing construction site keeps sending it.
    #[test]
    fn llm_request_default_does_not_omit_max_tokens() {
        let req = LlmRequest::default();
        assert!(!req.omit_max_tokens);
    }

    /// Contract §5.4: a full Sonar `search_results[]` element round-trips —
    /// all six fields, including the optional `date`/`last_updated`.
    #[test]
    fn flux_search_result_full_round_trip() {
        let raw = serde_json::json!({
            "title": "JWST snaps a new image",
            "url": "https://science.nasa.gov/jwst",
            "date": "2026-06-15",
            "last_updated": "2026-06-16",
            "snippet": "The telescope captured…",
            "source": "web"
        });
        let parsed: FluxSearchResult = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(parsed.title, "JWST snaps a new image");
        assert_eq!(parsed.url, "https://science.nasa.gov/jwst");
        assert_eq!(parsed.date.as_deref(), Some("2026-06-15"));
        assert_eq!(parsed.last_updated.as_deref(), Some("2026-06-16"));
        assert_eq!(parsed.snippet, "The telescope captured…");
        assert_eq!(parsed.source, "web");
        // Re-serialize and re-parse to prove a clean round-trip.
        let back: FluxSearchResult =
            serde_json::from_value(serde_json::to_value(&parsed).unwrap()).unwrap();
        assert_eq!(back, parsed);
    }

    /// Contract §5.4: `date`/`last_updated` are frequently absent on a given
    /// result — a card missing them must deserialize with `None`, not error.
    #[test]
    fn flux_search_result_missing_optionals_defaults_to_none() {
        let raw = serde_json::json!({
            "title": "t", "url": "u", "snippet": "s", "source": "web"
        });
        let parsed: FluxSearchResult = serde_json::from_value(raw).unwrap();
        assert!(parsed.date.is_none());
        assert!(parsed.last_updated.is_none());
        assert_eq!(parsed.url, "u");
    }

    #[test]
    fn llm_event_citations_carries_urls() {
        let event = LlmEvent::Citations(vec!["https://a.example".into()]);
        match event {
            LlmEvent::Citations(urls) => assert_eq!(urls, vec!["https://a.example".to_string()]),
            _ => panic!("expected Citations"),
        }
    }

    #[test]
    fn test_llm_event_tool_use_fields() {
        let event = LlmEvent::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({"cmd": "ls"}),
            extra: None,
        };
        match &event {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }
}
