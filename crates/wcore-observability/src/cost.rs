//! W6 F7 — per-turn USD cost estimation from token counts + `ProviderCompat`
//! price rows.
//!
//! This is the SINGLE source of cost arithmetic in the engine; the agent
//! engine consumes it via `estimate_turn_cost(...)` and writes the result
//! into `TurnTrace.cost_usd`. Prices live in `ProviderCompat` presets;
//! this module only does the multiplication.

use wcore_config::compat::ProviderCompat;

/// Compute the USD cost of one turn from raw token counts plus the
/// provider's price rows. Missing price rows are treated as zero.
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
    let input = input_tokens as f64 * compat.cost_per_input_token.unwrap_or(0.0);
    let output = output_tokens as f64 * compat.cost_per_output_token.unwrap_or(0.0);
    let cache_read = cache_read_tokens as f64 * compat.cost_per_cache_read_token.unwrap_or(0.0);
    let cache_write = cache_write_tokens as f64 * compat.cost_per_cache_write_token.unwrap_or(0.0);
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
}
