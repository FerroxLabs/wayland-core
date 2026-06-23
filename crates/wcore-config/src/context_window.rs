//! THE KERNEL — the single per-turn context-window computation.
//!
//! `% full` and the pre-flight overflow ceiling must be computed against the
//! window of the model that will ACTUALLY serve THIS request — i.e. the
//! post-swap effective model — not a stale `CompactConfig` default (the #255
//! "false context window size" bug, where a 200k default denominator survived
//! a Flux/tier swap down to a 128k model).
//!
//! There is exactly ONE division (in [`ContextWindow::fraction`]). Every other
//! consumer (the overflow guard today; the #279 gauge and #280 autocompact
//! trigger as follow-ons) derives its number from this struct and never
//! re-divides. The window is `Option<u64>` on purpose: an unknown model yields
//! `None`, which forbids fabricating a denominator and makes every downstream
//! consumer fail open rather than guard/display against a wrong number.
//!
//! Placement: this module lives in `wcore-config` next to
//! [`crate::limits::model_output_ceiling`], the only per-model window table in
//! the tree. Co-locating adds zero cross-crate edges and no dep cycle — the
//! kernel calls a sibling module. `wcore-agent` (overflow guard, autocompact
//! trigger) and `wcore-cli` (TUI) already depend on `wcore-config`.
//! `wcore-protocol` deliberately does NOT depend on `wcore-config`, so it
//! cannot call the kernel: protocol transports the computed integer percent as
//! an opaque serde number, matching the observability crate's decoupling.

use crate::limits::model_output_ceiling;

/// One turn's assembled-tokens-over-active-window view.
///
/// Construct once per turn via [`ContextWindow::resolve`] immediately AFTER the
/// model swap so `model` is the post-swap effective model. Recompute-on-swap is
/// therefore structural — there is no stored state to invalidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    /// Assembled input tokens for this request (`estimate_request_tokens`).
    pub used_tokens: u64,
    /// The active model's REAL context window. `None` = unknown model with no
    /// usable config fallback; downstream consumers fail open on `None`.
    pub window: Option<u64>,
}

impl ContextWindow {
    /// THE KERNEL. Resolve the active model's window for this turn.
    ///
    /// `provider` / `model` are the POST-swap effective values (the same pair
    /// fed to `size_output_cap`). A KNOWN model's real window
    /// ([`model_output_ceiling`]`.1`) ALWAYS wins — this is the #255 root-cause
    /// fix: a swapped-in gpt-4o (128k) must not be measured against a stale
    /// 200k default. `config_window` is a fail-open fallback used ONLY when the
    /// model is unknown AND the user supplied a positive override (their TOML
    /// `context_window`); when both are absent the window is `None` and no
    /// denominator is fabricated.
    pub fn resolve(used_tokens: u64, provider: &str, model: &str, config_window: u64) -> Self {
        let window = model_output_ceiling(provider, model)
            .map(|(_, ctx)| ctx as u64)
            .or(if config_window > 0 {
                Some(config_window)
            } else {
                None
            });
        ContextWindow {
            used_tokens,
            window,
        }
    }

    /// The ONLY division. `used / window`. `None` when the window is unknown or
    /// zero (defensive — `resolve` already refuses a zero fallback). Returns
    /// `> 1.0` on overflow on purpose (not clamped): the overflow guard relies
    /// on `used >= ceiling` firing and the gauge should show the truth.
    pub fn fraction(&self) -> Option<f64> {
        let w = self.window?;
        if w == 0 {
            return None;
        }
        Some(self.used_tokens as f64 / w as f64)
    }

    /// Integer percent full. Thin wrapper over [`fraction`](Self::fraction); no
    /// re-division. `> 100` on overflow (intentionally unclamped).
    pub fn percent(&self) -> Option<u32> {
        self.fraction().map(|f| (f * 100.0).round() as u32)
    }

    /// Pre-flight input ceiling = window − output_reserve − emergency_buffer,
    /// saturating (never underflows). `None` when the window is unknown — the
    /// overflow guard then SKIPS (fail open), identical to today's `window > 0`
    /// skip, with `size_output_cap`'s UNKNOWN_CAP + the provider 400 as backstops.
    pub fn input_ceiling(&self, output_reserve: u64, emergency_buffer: u64) -> Option<u64> {
        let w = self.window?;
        Some(
            w.saturating_sub(output_reserve)
                .saturating_sub(emergency_buffer),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_model_uses_real_window_not_config() {
        // #255 root-cause assertion: a KNOWN model overrides the stale 200k
        // config default. gpt-4o-mini's real window is 128k.
        let ctx = ContextWindow::resolve(1_000, "openai", "gpt-4o-mini", 200_000);
        assert_eq!(ctx.window, Some(128_000));
    }

    #[test]
    fn resolve_unknown_model_falls_back_to_config() {
        // flux-auto is unlisted -> fail open to the user/config value, NOT a
        // hardcoded 200k inside the kernel.
        let ctx = ContextWindow::resolve(1_000, "flux-router", "flux-auto", 200_000);
        assert_eq!(ctx.window, Some(200_000));
    }

    #[test]
    fn resolve_unknown_model_and_zero_config_is_none() {
        // No real window and no positive config -> no fabricated denominator.
        let ctx = ContextWindow::resolve(1_000, "flux-router", "flux-auto", 0);
        assert_eq!(ctx.window, None);
    }

    #[test]
    fn known_model_wins_over_differing_config_window() {
        // A TOML context_window=200_000 must NOT override the real 128k of a
        // swapped gpt-4o (the #255 fix). config_window is fallback-only.
        let ctx = ContextWindow::resolve(1_000, "openai", "gpt-4o", 200_000);
        assert_eq!(ctx.window, Some(128_000));
    }

    #[test]
    fn fraction_and_percent_basic() {
        let ctx = ContextWindow {
            used_tokens: 64_000,
            window: Some(128_000),
        };
        assert_eq!(ctx.fraction(), Some(0.5));
        assert_eq!(ctx.percent(), Some(50));
    }

    #[test]
    fn fraction_overflow_exceeds_one_not_clamped() {
        // 250k against gpt-4o's 128k -> > 100% shown, not hidden.
        let ctx = ContextWindow {
            used_tokens: 250_000,
            window: Some(128_000),
        };
        assert!(ctx.fraction().unwrap() > 1.0);
        assert_eq!(ctx.percent(), Some(195));
    }

    #[test]
    fn fraction_unknown_window_is_none() {
        let ctx = ContextWindow {
            used_tokens: 1_000,
            window: None,
        };
        assert_eq!(ctx.fraction(), None);
        assert_eq!(ctx.percent(), None);
    }

    #[test]
    fn fraction_zero_tokens() {
        let ctx = ContextWindow {
            used_tokens: 0,
            window: Some(128_000),
        };
        assert_eq!(ctx.fraction(), Some(0.0));
        assert_eq!(ctx.percent(), Some(0));
    }

    #[test]
    fn fraction_zero_window_no_div_by_zero() {
        // Defensive: even if a zero window reaches the struct, no panic.
        let ctx = ContextWindow {
            used_tokens: 1_000,
            window: Some(0),
        };
        assert_eq!(ctx.fraction(), None);
    }

    #[test]
    fn input_ceiling_known_fires_on_gpt4o_where_200k_would_not() {
        let ctx = ContextWindow {
            used_tokens: 110_000,
            window: Some(128_000),
        };
        let ceiling = ctx.input_ceiling(20_000, 3_000);
        assert_eq!(ceiling, Some(105_000));
        // 110_000 >= 105_000 -> the #255 guard fires; the old 200k-based
        // ceiling (177_000) would have let it through (false negative).
        assert!(ctx.used_tokens >= ceiling.unwrap());
    }

    #[test]
    fn input_ceiling_unknown_is_none() {
        let ctx = ContextWindow {
            used_tokens: 110_000,
            window: None,
        };
        assert_eq!(ctx.input_ceiling(20_000, 3_000), None);
    }

    #[test]
    fn input_ceiling_saturates_no_underflow() {
        let ctx = ContextWindow {
            used_tokens: 0,
            window: Some(1_000),
        };
        assert_eq!(ctx.input_ceiling(20_000, 3_000), Some(0));
    }
}
