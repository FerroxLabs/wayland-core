//! Per-message-shape smart model routing.
//!
//! # Status: WIRED (v0.6.3 D.1 Round 1 cleanup)
//!
//! All types + the [`select_tier`] / [`route`] functions compile and test.
//! v0.6.3 closes the producer/consumer gap: `wcore-agent`'s engine builds a
//! [`RequestShape`] each turn, calls [`route`], and stamps the resulting
//! label onto `LlmRequest::routing_hint` via [`RoutingDecision::to_hint`];
//! [`crate::ProviderChain::stream`] reads that hint and surfaces it in a
//! `tracing` span for dispatch observability.
//!
//! Note: the engine-side producer populates `input_tokens`,
//! `max_output_tokens`, `tool_call_count`, and `requires_vision` from real
//! request data; `code_ratio` is left at a conservative `0.0` (a real
//! code-ratio scanner is out of scope), so this producer never emits the
//! `code_heavy`/`Balanced` decision — the vision/large-context/tool-heavy/
//! simple decisions are all genuine.
//!
//! Given a [`RequestShape`] (token count, code-heaviness, tool-use prevalence,
//! vision requirement), [`select_tier`] picks one of three tiers
//! ([`RoutingTier::Cheap`], [`RoutingTier::Balanced`], [`RoutingTier::Premium`]).
//!
//! The higher-level [`route`] returns a [`RoutingDecision`] that names the rule
//! that fired, so the picker's behavior is visible in trace observability.
//!
//! Ported from the prior Wayland Python engine's smart model routing. The Python
//! version routed by parsing the raw user message; this Rust port lifts the
//! decision to a typed request shape so callers — provider chain, agent
//! engine — can supply structured inputs (token counts, vision flag) instead
//! of doing keyword matching at this layer.

use serde::{Deserialize, Serialize};

/// Coarse model tier selected by [`select_tier`].
///
/// The variants are intentionally provider-agnostic; mapping a tier to a
/// concrete model is the caller's job (typically the provider chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingTier {
    /// Simple/short turns — small, fast, cheapest model.
    Cheap,
    /// Code-heavy turns that don't trip the premium thresholds.
    Balanced,
    /// Vision, large-context, or tool-heavy turns — the strongest model.
    Premium,
}

/// Structured view of the request the router sees.
///
/// Fields are deliberately neutral (no message text, no provider details) so
/// upstream code can populate them from any source — token estimator,
/// content-block scanner, tool-call history.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RequestShape {
    /// Estimated input token count for the prompt the router is shaping.
    pub input_tokens: usize,
    /// Caller's `max_tokens` for the response.
    pub max_output_tokens: usize,
    /// Fraction of the prompt that looks like code, in `[0.0, 1.0]`.
    pub code_ratio: f32,
    /// Number of tool calls the prompt history already contains.
    pub tool_call_count: u32,
    /// Whether the request includes image / vision content blocks.
    pub requires_vision: bool,
}

/// Tunable thresholds for [`select_tier`].
///
/// [`Default`] returns conservative values calibrated to keep the cheap tier
/// reserved for genuinely simple turns:
/// - `code_threshold = 0.30` (≥30 % code → balanced)
/// - `large_context_tokens = 8000` (strictly above → premium)
/// - `tool_heavy_calls = 3` (at or above → premium)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RoutingHeuristics {
    /// Code ratio at or above which a request is promoted to
    /// [`RoutingTier::Balanced`].
    pub code_threshold: f32,
    /// Input-token count above which a request is promoted to
    /// [`RoutingTier::Premium`] (strict greater-than).
    pub large_context_tokens: usize,
    /// Tool-call count at or above which a request is promoted to
    /// [`RoutingTier::Premium`] (greater-than-or-equal).
    pub tool_heavy_calls: u32,
}

impl Default for RoutingHeuristics {
    fn default() -> Self {
        Self {
            code_threshold: 0.30,
            large_context_tokens: 8000,
            tool_heavy_calls: 3,
        }
    }
}

/// Outcome of [`route`] — tier plus the rule name that produced it.
///
/// The `reason` is a static string so it can be cheaply embedded in trace
/// spans / metric labels without allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub tier: RoutingTier,
    pub reason: &'static str,
}

impl RoutingDecision {
    /// Convert this decision into a stable [`RoutingHint`] label for
    /// stamping onto an `LlmRequest`. The label is `"{tier}:{reason}"`
    /// (e.g. `"premium:large_context"`) — a free-form string that
    /// downstream providers consult opportunistically and ignore when
    /// unrecognized.
    pub fn to_hint(&self) -> wcore_types::llm::RoutingHint {
        let tier = match self.tier {
            RoutingTier::Cheap => "cheap",
            RoutingTier::Balanced => "balanced",
            RoutingTier::Premium => "premium",
        };
        wcore_types::llm::RoutingHint::new(format!("{tier}:{}", self.reason))
    }
}

/// Known reason strings emitted by [`route`].
///
/// Exposed so observability / test code can match against a closed set
/// instead of re-typing string literals.
pub mod reasons {
    pub const VISION: &str = "requires_vision";
    pub const LARGE_CONTEXT: &str = "large_context";
    pub const TOOL_HEAVY: &str = "tool_heavy";
    pub const CODE_HEAVY: &str = "code_heavy";
    pub const SIMPLE: &str = "simple";
}

/// Pick a [`RoutingTier`] for the given request shape.
///
/// Rule order (first match wins):
/// 1. `requires_vision` → [`RoutingTier::Premium`]
/// 2. `input_tokens > large_context_tokens` → [`RoutingTier::Premium`]
/// 3. `tool_call_count >= tool_heavy_calls` → [`RoutingTier::Premium`]
/// 4. `code_ratio >= code_threshold` → [`RoutingTier::Balanced`]
/// 5. otherwise → [`RoutingTier::Cheap`]
pub fn select_tier(shape: &RequestShape, h: &RoutingHeuristics) -> RoutingTier {
    route(shape, h).tier
}

/// Like [`select_tier`] but also returns the rule that fired.
///
/// Use this when you want the decision to be visible in trace spans.
pub fn route(shape: &RequestShape, h: &RoutingHeuristics) -> RoutingDecision {
    if shape.requires_vision {
        return RoutingDecision {
            tier: RoutingTier::Premium,
            reason: reasons::VISION,
        };
    }
    if shape.input_tokens > h.large_context_tokens {
        return RoutingDecision {
            tier: RoutingTier::Premium,
            reason: reasons::LARGE_CONTEXT,
        };
    }
    if shape.tool_call_count >= h.tool_heavy_calls {
        return RoutingDecision {
            tier: RoutingTier::Premium,
            reason: reasons::TOOL_HEAVY,
        };
    }
    if shape.code_ratio >= h.code_threshold {
        return RoutingDecision {
            tier: RoutingTier::Balanced,
            reason: reasons::CODE_HEAVY,
        };
    }
    RoutingDecision {
        tier: RoutingTier::Cheap,
        reason: reasons::SIMPLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_shape() -> RequestShape {
        RequestShape {
            input_tokens: 100,
            max_output_tokens: 1024,
            code_ratio: 0.0,
            tool_call_count: 0,
            requires_vision: false,
        }
    }

    #[test]
    fn vision_request_routes_premium() {
        let mut shape = base_shape();
        shape.requires_vision = true;
        let decision = route(&shape, &RoutingHeuristics::default());
        assert_eq!(decision.tier, RoutingTier::Premium);
        assert_eq!(decision.reason, reasons::VISION);
    }

    #[test]
    fn large_context_routes_premium() {
        let h = RoutingHeuristics::default();
        let mut shape = base_shape();
        shape.input_tokens = h.large_context_tokens + 1;
        let decision = route(&shape, &h);
        assert_eq!(decision.tier, RoutingTier::Premium);
        assert_eq!(decision.reason, reasons::LARGE_CONTEXT);
    }

    #[test]
    fn tool_heavy_routes_premium() {
        let h = RoutingHeuristics::default();
        let mut shape = base_shape();
        shape.tool_call_count = h.tool_heavy_calls;
        let decision = route(&shape, &h);
        assert_eq!(decision.tier, RoutingTier::Premium);
        assert_eq!(decision.reason, reasons::TOOL_HEAVY);
    }

    #[test]
    fn code_heavy_routes_balanced() {
        let h = RoutingHeuristics::default();
        let mut shape = base_shape();
        shape.code_ratio = 0.75;
        let decision = route(&shape, &h);
        assert_eq!(decision.tier, RoutingTier::Balanced);
        assert_eq!(decision.reason, reasons::CODE_HEAVY);
    }

    #[test]
    fn simple_chat_routes_cheap() {
        let decision = route(&base_shape(), &RoutingHeuristics::default());
        assert_eq!(decision.tier, RoutingTier::Cheap);
        assert_eq!(decision.reason, reasons::SIMPLE);
    }

    #[test]
    fn boundary_at_code_threshold_inclusive() {
        let h = RoutingHeuristics::default();
        let mut shape = base_shape();
        // exactly at the threshold counts as code-heavy (>=)
        shape.code_ratio = h.code_threshold;
        assert_eq!(select_tier(&shape, &h), RoutingTier::Balanced);
        // just below the threshold remains cheap
        shape.code_ratio = h.code_threshold - 0.0001;
        assert_eq!(select_tier(&shape, &h), RoutingTier::Cheap);
    }

    #[test]
    fn boundary_at_large_context_exclusive() {
        let h = RoutingHeuristics::default();
        let mut shape = base_shape();
        // exactly at the threshold is NOT premium (strict greater-than)
        shape.input_tokens = h.large_context_tokens;
        assert_eq!(select_tier(&shape, &h), RoutingTier::Cheap);
        // one token above flips to premium
        shape.input_tokens = h.large_context_tokens + 1;
        assert_eq!(select_tier(&shape, &h), RoutingTier::Premium);
    }

    #[test]
    fn route_returns_reason_string() {
        let h = RoutingHeuristics::default();
        let cases = [
            {
                let mut s = base_shape();
                s.requires_vision = true;
                s
            },
            {
                let mut s = base_shape();
                s.input_tokens = h.large_context_tokens + 1;
                s
            },
            {
                let mut s = base_shape();
                s.tool_call_count = h.tool_heavy_calls;
                s
            },
            {
                let mut s = base_shape();
                s.code_ratio = 0.5;
                s
            },
            base_shape(),
        ];
        let known = [
            reasons::VISION,
            reasons::LARGE_CONTEXT,
            reasons::TOOL_HEAVY,
            reasons::CODE_HEAVY,
            reasons::SIMPLE,
        ];
        for shape in &cases {
            let decision = route(shape, &h);
            assert!(
                known.contains(&decision.reason),
                "unexpected reason: {}",
                decision.reason
            );
        }
    }

    #[test]
    fn serde_routing_tier_roundtrip() {
        for tier in [
            RoutingTier::Cheap,
            RoutingTier::Balanced,
            RoutingTier::Premium,
        ] {
            let json = serde_json::to_string(&tier).expect("serialize");
            let back: RoutingTier = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(tier, back);
        }
        // lowercase rename — guard against accidental rename in the enum attr
        assert_eq!(
            serde_json::to_string(&RoutingTier::Premium).unwrap(),
            "\"premium\""
        );
    }

    #[test]
    fn decision_to_hint_formats_tier_and_reason() {
        let mut shape = base_shape();
        shape.requires_vision = true;
        let hint = route(&shape, &RoutingHeuristics::default()).to_hint();
        assert_eq!(hint.0, "premium:requires_vision");

        let hint = route(&base_shape(), &RoutingHeuristics::default()).to_hint();
        assert_eq!(hint.0, "cheap:simple");

        let mut shape = base_shape();
        shape.code_ratio = 0.9;
        let hint = route(&shape, &RoutingHeuristics::default()).to_hint();
        assert_eq!(hint.0, "balanced:code_heavy");
    }

    #[test]
    fn default_heuristics_have_sensible_values() {
        let h = RoutingHeuristics::default();
        assert!(
            h.code_threshold > 0.0 && h.code_threshold < 1.0,
            "code_threshold must be a fraction"
        );
        assert!(
            h.large_context_tokens >= 1000,
            "large_context_tokens must be a non-trivial budget"
        );
        assert!(
            h.tool_heavy_calls >= 2,
            "tool_heavy_calls must allow at least one tool call before tripping"
        );
    }
}
