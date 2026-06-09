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
    /// Error from the API
    Error(String),
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
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::MaxTurns),
            FinishReason::Error
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
