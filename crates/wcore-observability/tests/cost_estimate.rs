//! W6 F7 cost estimate unit tests — keep ProviderCompat as the single source
//! of truth for pricing. These tests pin the math, not the prices (prices
//! live in compat.rs presets and may change quarterly).

use wcore_config::compat::ProviderCompat;
use wcore_observability::cost::estimate_turn_cost;

#[test]
fn cost_zero_when_compat_has_no_rows() {
    let compat = ProviderCompat::default();
    let cost = estimate_turn_cost(1000, 500, 100, 200, &compat);
    assert_eq!(cost, 0.0);
}

#[test]
fn cost_uses_input_and_output_rows() {
    let compat = ProviderCompat {
        cost_per_input_token: Some(0.00001),  // $10/M
        cost_per_output_token: Some(0.00003), // $30/M
        ..ProviderCompat::default()
    };
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    // 1000*0.00001 + 500*0.00003 = 0.01 + 0.015 = 0.025
    assert!((cost - 0.025).abs() < 1e-9, "cost was {cost}");
}

#[test]
fn cost_includes_cache_when_set() {
    let compat = ProviderCompat {
        cost_per_input_token: Some(0.00001),
        cost_per_output_token: Some(0.00003),
        cost_per_cache_read_token: Some(0.000001),
        cost_per_cache_write_token: Some(0.0000125),
        ..ProviderCompat::default()
    };
    let cost = estimate_turn_cost(1000, 500, 5000, 2000, &compat);
    // 1000*0.00001 + 500*0.00003 + 5000*0.000001 + 2000*0.0000125
    //   = 0.01 + 0.015 + 0.005 + 0.025 = 0.055
    assert!((cost - 0.055).abs() < 1e-9, "cost was {cost}");
}

#[test]
fn cost_anthropic_preset_smoke() {
    // Doesn't pin specific dollar values (prices drift); only asserts non-zero.
    let compat = ProviderCompat::anthropic_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert!(cost > 0.0, "anthropic preset must produce non-zero cost");
}

#[test]
fn cost_bedrock_preset_smoke() {
    let compat = ProviderCompat::bedrock_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert!(cost > 0.0, "bedrock preset must produce non-zero cost");
}

#[test]
fn cost_openai_preset_is_zero() {
    // Per the 2026-05-24 pricing audit, `openai_defaults()` deliberately
    // returns $0/$0 sentinel rates instead of GPT-class prices — the real
    // model price rows live in pricing.toml and resolve before this
    // fallback. An OpenAI model that does NOT match the catalog reports
    // honest $0 (unknown) instead of the prior confident-but-wrong rate
    // (which silently 53x-overcharged for gpt-4o-mini and similar).
    let compat = ProviderCompat::openai_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert_eq!(
        cost, 0.0,
        "openai preset must report $0 sentinel (real prices come from pricing.toml)"
    );
}

#[test]
fn cost_vertex_preset_smoke() {
    let compat = ProviderCompat::vertex_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert!(cost > 0.0, "vertex preset must produce non-zero cost");
}

#[test]
fn cost_ollama_preset_is_zero() {
    // Local provider preset must price-to-zero so users see real cost
    // savings vs. cloud providers.
    let compat = ProviderCompat::ollama_defaults();
    let cost = estimate_turn_cost(1000, 500, 0, 0, &compat);
    assert_eq!(cost, 0.0);
}

#[test]
fn cost_partial_rows_only_charges_set_categories() {
    // Only input price set; output/cache rows are None and default to 0.
    let compat = ProviderCompat {
        cost_per_input_token: Some(0.00002),
        ..ProviderCompat::default()
    };
    let cost = estimate_turn_cost(1000, 500, 100, 100, &compat);
    assert!((cost - 0.02).abs() < 1e-9, "cost was {cost}");
}
