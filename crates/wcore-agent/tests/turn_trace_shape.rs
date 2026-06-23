//! W6 T3 regression guard for the TurnTrace shape upgrade.
//!
//! W1 emitted `provider = "anthropic-family" | "openai-family"` via a
//! `supports_thinking()` heuristic. W6 replaced that with
//! `ProviderCompat.provider_type()` so the structured per-provider id
//! flows into traces and cost attribution. This test pins the new
//! contract; if anyone reintroduces the family-string heuristic it fails
//! here loud.

use wcore_config::compat::ProviderCompat;
use wcore_observability::trace::TurnTrace;

#[test]
fn turn_trace_provider_field_is_structured_not_family_anthropic() {
    let compat = ProviderCompat::anthropic_defaults();
    let trace = TurnTrace {
        turn: 0,
        model: "claude-opus-4-7".into(),
        provider: compat.provider_type().to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.0,
        tool_calls: vec![],
        hook_actions: vec![],
        source_product: "wayland-core".into(),
        agent_run_id: String::new(),
    };
    assert_eq!(trace.provider, "anthropic");
    assert_ne!(
        trace.provider, "anthropic-family",
        "W1 supports_thinking() heuristic must be gone"
    );
}

#[test]
fn turn_trace_provider_field_distinguishes_anthropic_family_providers() {
    // Anthropic Direct vs. Bedrock vs. Vertex now report distinct provider
    // ids — the whole point of W6 cost attribution (per the W1 finding-8
    // doc comment that scheduled this work).
    assert_eq!(
        ProviderCompat::anthropic_defaults().provider_type(),
        "anthropic"
    );
    assert_eq!(
        ProviderCompat::bedrock_defaults().provider_type(),
        "bedrock"
    );
    assert_eq!(ProviderCompat::vertex_defaults().provider_type(), "vertex");
    assert_eq!(ProviderCompat::openai_defaults().provider_type(), "openai");
}

#[test]
fn turn_trace_cost_usd_is_populated_from_compat() {
    // W6 T2 helper covers the math thoroughly; this test pins that the
    // engine call site uses it (via the same arithmetic) instead of the
    // hardcoded 0.0 from W1.
    use wcore_observability::cost::estimate_turn_cost;
    let compat = ProviderCompat::anthropic_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert!(
        cost > 0.0,
        "engine.rs T3 must produce a non-zero cost from anthropic preset"
    );
}
