//! W6 F7 — per-turn USD cost estimation from token counts + `ProviderCompat`
//! price rows.
//!
//! This is the SINGLE source of cost arithmetic in the engine; the agent
//! engine consumes it via `estimate_turn_cost(...)` and writes the result
//! into `TurnTrace.cost_usd`. Prices live in `ProviderCompat` presets;
//! this module only does the multiplication.

use wcore_config::compat::ProviderCompat;

/// Compute the USD cost of one turn from raw token counts plus the
/// provider's price rows. Missing cache-category rows fall back to the normal
/// input rate so cached tokens cannot silently become free; a completely
/// unpriced compatibility profile still evaluates to zero.
///
/// Pricing is `tokens * price_per_token` for each of the four token
/// categories (input, output, cache_read, cache_write). When `ProviderCompat`
/// has no price rows (the `default()` state, or a custom provider that
/// hasn't been populated), this returns `0.0` — preserving the W1 default
/// behaviour exactly.
pub fn estimate_turn_cost(
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    compat: &ProviderCompat,
) -> f64 {
    let input_rate = compat.cost_per_input_token.unwrap_or(0.0);
    let input = input_tokens as f64 * input_rate;
    let output = output_tokens as f64 * compat.cost_per_output_token.unwrap_or(0.0);
    let cache_read_rate = compat
        .cost_per_cache_read_token
        .filter(|rate| *rate > 0.0)
        .unwrap_or(input_rate);
    let cache_write_rate = compat
        .cost_per_cache_write_token
        .filter(|rate| *rate > 0.0)
        .unwrap_or(input_rate);
    let cache_read = cache_read_tokens as f64 * cache_read_rate;
    let cache_write = cache_write_tokens as f64 * cache_write_rate;
    input + output + cache_read + cache_write
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_tokens_zero_cost() {
        let compat = ProviderCompat::anthropic_defaults();
        assert_eq!(estimate_turn_cost(0, 0, 0, 0, &compat), 0.0);
    }

    #[test]
    fn default_compat_zero_cost() {
        let compat = ProviderCompat::default();
        assert_eq!(estimate_turn_cost(1000, 500, 100, 200, &compat), 0.0);
    }

    #[test]
    fn missing_cache_rates_fall_back_to_input_rate() {
        let compat = ProviderCompat {
            cost_per_input_token: Some(0.000_001),
            ..Default::default()
        };
        assert_eq!(estimate_turn_cost(0, 0, 1_000, 2_000, &compat), 0.003);
    }

    #[test]
    fn zero_cache_rate_sentinels_fall_back_to_input_rate() {
        let compat = ProviderCompat {
            cost_per_input_token: Some(0.000_001),
            cost_per_cache_read_token: Some(0.0),
            cost_per_cache_write_token: Some(0.0),
            ..Default::default()
        };
        assert_eq!(estimate_turn_cost(0, 0, 1_000, 2_000, &compat), 0.003);
    }
}
