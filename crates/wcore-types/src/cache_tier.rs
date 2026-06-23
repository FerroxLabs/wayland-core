//! Anthropic prompt cache tier picker (5m vs 1h ephemeral cache_control).
//!
//! Ported from an upstream MIT-licensed library's (see THIRD-PARTY-NOTICES.md)
//! `apply_anthropic_cache_control`.
//!
//! The predecessor hard-codes `cache_ttl: "5m"` or `"1h"` at call sites. This
//! module lifts the *selection* of the tier into a pure decision function so the
//! Anthropic adapter (`anthropic.rs`) can choose the right marker per request
//! based on expected reuse window and prompt size.
//!
//! Per Anthropic prompt-caching docs:
//!   - Minimum cacheable block: 1024 tokens (Sonnet/Opus). Below this the
//!     `cache_control` marker is rejected, so we return `None` to skip it.
//!   - `ephemeral` markers default to a 5-minute TTL; passing `ttl: "1h"`
//!     opts into the 1-hour tier (priced higher per write, cheaper per reuse
//!     if the reuse window exceeds 5 minutes).
//!
//! Wiring into `AnthropicProvider::stream` lands in T3-8; this crate exposes
//! only the picker so it can be unit-tested in isolation.

/// Which ephemeral cache tier (if any) to attach to a prompt block.
///
/// # Status (v0.6.2)
///
/// `Ephemeral5m` is wired end-to-end through `AnthropicProvider::stream` ->
/// `apply_cache_zones` and works in production. `Ephemeral1h` is SCAFFOLDED:
/// the variant compiles, tests cover the picker, but `apply_cache_zones`
/// hard-codes `Ephemeral5m` because `LlmRequest` has no `cache_tier:
/// Option<CacheTier>` field for callers to express a preference. Wiring
/// the field through `LlmRequest` + `apply_cache_zones` is deferred to
/// v0.6.3+.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    /// Default 5-minute ephemeral cache. Cheapest write, shortest lifetime.
    Ephemeral5m,
    /// 1-hour ephemeral cache (`ttl: "1h"`). More expensive write but cheaper
    /// per reuse when the same prefix is hit again within the hour.
    ///
    /// **Scaffolded — not yet reachable in production.** See enum-level docs
    /// and R2 fix B1. `apply_cache_zones` does not consult this variant in
    /// v0.6.2; the picker function `pick_with_config` is unit-tested only.
    Ephemeral1h,
    /// Skip cache_control entirely (prompt too small, or feature disabled).
    None,
}

impl CacheTier {
    /// Stable string form for telemetry / cache_control payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheTier::Ephemeral5m => "5m",
            CacheTier::Ephemeral1h => "1h",
            CacheTier::None => "none",
        }
    }
}

/// Tunable thresholds for [`pick_with_config`].
///
/// Defaults match Anthropic's published minimums and the predecessor's implicit
/// "longer than 5 minutes -> 1h" heuristic.
#[derive(Debug, Clone, Copy)]
pub struct CacheTierConfig {
    /// Minimum input tokens before cache_control is worth attaching.
    /// Anthropic rejects cache markers below 1024 tokens on Sonnet/Opus.
    pub min_tokens: usize,
    /// Reuse window (seconds) above which the 1h tier is preferred over 5m.
    /// Inclusive boundary: `expected_reuse_window_seconds > five_min_threshold_secs`
    /// promotes to 1h; equal-to stays on 5m.
    pub five_min_threshold_secs: u64,
}

impl Default for CacheTierConfig {
    fn default() -> Self {
        Self {
            min_tokens: 1024,
            five_min_threshold_secs: 300,
        }
    }
}

/// Pick the appropriate cache tier given prompt size and expected reuse window.
///
/// Returns [`CacheTier::None`] when the prompt is too small to cache.
/// Returns [`CacheTier::Ephemeral1h`] when reuse is expected beyond 5 minutes.
/// Returns [`CacheTier::Ephemeral5m`] otherwise.
pub fn pick_cache_tier(input_tokens: usize, expected_reuse_window_seconds: u64) -> CacheTier {
    pick_with_config(
        input_tokens,
        expected_reuse_window_seconds,
        CacheTierConfig::default(),
    )
}

/// Configurable variant of [`pick_cache_tier`] for tests and custom tuning.
pub fn pick_with_config(
    input_tokens: usize,
    expected_reuse_window_seconds: u64,
    cfg: CacheTierConfig,
) -> CacheTier {
    if input_tokens < cfg.min_tokens {
        return CacheTier::None;
    }
    if expected_reuse_window_seconds > cfg.five_min_threshold_secs {
        CacheTier::Ephemeral1h
    } else {
        CacheTier::Ephemeral5m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_minimum_tokens_returns_none() {
        assert_eq!(pick_cache_tier(500, 30), CacheTier::None);
    }

    #[test]
    fn short_reuse_returns_5m() {
        assert_eq!(pick_cache_tier(2000, 60), CacheTier::Ephemeral5m);
    }

    #[test]
    fn long_reuse_returns_1h() {
        assert_eq!(pick_cache_tier(2000, 1800), CacheTier::Ephemeral1h);
    }

    #[test]
    fn exactly_at_min_token_threshold() {
        // 1024 is the inclusive lower bound -- cacheable.
        assert_eq!(pick_cache_tier(1024, 60), CacheTier::Ephemeral5m);
    }

    #[test]
    fn exactly_at_5m_boundary() {
        // 300 is NOT > 300 -> stays on 5m tier.
        assert_eq!(pick_cache_tier(2000, 300), CacheTier::Ephemeral5m);
    }

    #[test]
    fn just_over_5m_boundary() {
        assert_eq!(pick_cache_tier(2000, 301), CacheTier::Ephemeral1h);
    }

    #[test]
    fn as_str_round_trip() {
        assert_eq!(CacheTier::Ephemeral5m.as_str(), "5m");
        assert_eq!(CacheTier::Ephemeral1h.as_str(), "1h");
        assert_eq!(CacheTier::None.as_str(), "none");
    }

    #[test]
    fn custom_config_overrides_min() {
        let cfg = CacheTierConfig {
            min_tokens: 256,
            five_min_threshold_secs: 300,
        };
        // 500 tokens would be None under default 1024 min, but cacheable here.
        assert_eq!(pick_with_config(500, 60, cfg), CacheTier::Ephemeral5m);
    }

    #[test]
    fn custom_config_overrides_threshold() {
        let cfg = CacheTierConfig {
            min_tokens: 1024,
            five_min_threshold_secs: 60,
        };
        // 120s reuse is short under default 300s threshold, but long here.
        assert_eq!(pick_with_config(2000, 120, cfg), CacheTier::Ephemeral1h);
    }
}
