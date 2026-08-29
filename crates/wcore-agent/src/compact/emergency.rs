//! Emergency truncation: the last safety net before a context overflow.
//!
//! When `last_input_tokens` is within `emergency_buffer` of the full
//! `context_window`, the engine should block the next API call and ask
//! the user to compact or start a new conversation.
//!
//! Unlike autocompact, the emergency check always applies — even when
//! the compaction system is disabled via `CompactConfig.enabled`.

use wcore_config::compact::CompactConfig;

/// User-facing message shown when the emergency limit is hit.
pub const EMERGENCY_USER_MESSAGE: &str =
    "Context window nearly full. Please use /compact or start a new conversation.";

/// Check whether the last observed input token count has reached the
/// emergency blocking limit.
///
/// The limit is `effective_context_window - emergency_buffer`.  When
/// `last_input_tokens >= limit`, the engine must not send another API
/// request — doing so would almost certainly fail with a prompt-too-long
/// error from the provider.
///
/// This check is independent of `CompactConfig.enabled`; the emergency
/// safety net is always active.
///
/// `provider` / `model` are the POST-swap effective pair — see
/// [`emergency_limit`].
pub fn is_at_emergency_limit(
    last_input_tokens: u64,
    config: &CompactConfig,
    provider: &str,
    model: &str,
) -> bool {
    last_input_tokens as usize >= emergency_limit(config, provider, model)
}

/// The emergency hard-stop limit in tokens:
/// `effective_context_window - emergency_buffer`.
///
/// F23-04 exposes this so the cache/compaction ledger can report token pressure
/// as a distance from a real boundary rather than as a bare token count. It is
/// the SAME arithmetic [`is_at_emergency_limit`] tests against — extracted, not
/// re-derived, so the reported limit can never drift from the enforced one.
/// GH#635 extends that guarantee to the DENOMINATOR: the window comes from
/// [`CompactConfig::effective_context_window`] inside this function, so a
/// reporter and an enforcer cannot end up on different windows.
///
/// `provider` / `model` must be the POST-swap effective pair (the same values
/// fed to `size_output_cap` and the #255 pre-flight guard).
pub fn emergency_limit(config: &CompactConfig, provider: &str, model: &str) -> usize {
    config.emergency_limit_for_window(config.effective_context_window(provider, model))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1150 note: the window is PINNED here. These cases specify the buffer
    /// ARITHMETIC, and they used to get 200,000 by accident, from the
    /// unlisted-model fallback. That fallback is now the conservative
    /// `UNVERIFIED_CONTEXT_WINDOW`, so the pin states out loud the window the
    /// numbers below were written against.
    fn default_config() -> CompactConfig {
        CompactConfig {
            context_window: Some(200_000),
            ..CompactConfig::default()
        }
    }

    /// A provider/model pair the `wcore_config::limits` registry does NOT
    /// know, so the effective window is the configured fallback and these
    /// cases exercise the arithmetic rather than the registry.
    const UNKNOWN_PROVIDER: &str = "test-provider";
    const UNKNOWN_MODEL: &str = "test-model";

    // ── is_at_emergency_limit ──────────────────────────────────────────

    #[test]
    fn below_limit_returns_false() {
        // limit = 200k - 3k = 197k; 190k < 197k
        let config = default_config();
        assert!(!is_at_emergency_limit(
            190_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn above_limit_returns_true() {
        // 198k >= 197k
        let config = default_config();
        assert!(is_at_emergency_limit(
            198_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn at_exact_limit_returns_true() {
        // 197k >= 197k
        let config = default_config();
        assert!(is_at_emergency_limit(
            197_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn small_context_window() {
        let config = CompactConfig {
            context_window: Some(8_000),
            emergency_buffer: 3_000,
            ..default_config()
        };
        // limit = 8k - 3k = 5k; 6k >= 5k
        assert!(is_at_emergency_limit(
            6_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn zero_tokens_below_limit() {
        let config = default_config();
        assert!(!is_at_emergency_limit(
            0,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn custom_emergency_buffer() {
        let config = CompactConfig {
            context_window: Some(100_000),
            emergency_buffer: 10_000,
            ..default_config()
        };
        // limit = 100k - 10k = 90k
        assert!(!is_at_emergency_limit(
            89_999,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
        assert!(is_at_emergency_limit(
            90_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
        assert!(is_at_emergency_limit(
            95_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn works_regardless_of_enabled_flag() {
        let config = CompactConfig {
            enabled: false,
            ..default_config()
        };
        // Emergency check ignores the enabled flag
        assert!(is_at_emergency_limit(
            198_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn emergency_buffer_larger_than_context_window_saturates() {
        let config = CompactConfig {
            context_window: Some(1_000),
            emergency_buffer: 5_000,
            ..default_config()
        };
        // saturating_sub: limit = 0; any positive token count triggers
        assert!(is_at_emergency_limit(
            1,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
        // 0 tokens = 0 >= 0 → true (degenerate but safe)
        assert!(is_at_emergency_limit(
            0,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    // ── GH#635: the hard stop follows the MODEL's window ────────────────

    /// A registry-known 1.05M-window model must not hard-stop at ~197k.
    /// The real limit is 1_050_000 − 3_000 = 1_047_000.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: change
    /// `config.effective_context_window(provider, model)` back to
    /// `config.context_window` in `emergency_limit` (emergency.rs) — the
    /// limit collapses to 197_000 and the 197k assertion below fires, killing
    /// a session with 850k tokens of headroom left.
    #[test]
    fn large_window_model_does_not_hard_stop_at_197k() {
        // #1150: deliberately UNPINNED — this case is about the registry
        // window beating the fallback, and an operator `context_window`
        // outranks the registry.
        let config = CompactConfig::default();
        assert_eq!(
            emergency_limit(&config, "openai-chatgpt", "gpt-5.4"),
            1_047_000
        );
        assert!(!is_at_emergency_limit(
            197_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
        assert!(is_at_emergency_limit(
            1_047_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
    }

    /// A registry-known SMALL model hard-stops EARLIER than the default —
    /// the fix must lower boundaries as readily as it raises them, or it is
    /// just a blanket raise that trades premature compaction for 400s.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: same line as above; the raw
    /// `config.context_window` yields 197_000 and a 126k gpt-4o request is
    /// waved through to a provider 400.
    #[test]
    fn small_window_model_hard_stops_earlier_than_the_default() {
        // #1150: deliberately UNPINNED — this case is about the registry
        // window beating the fallback, and an operator `context_window`
        // outranks the registry.
        let config = CompactConfig::default();
        // 128_000 − 3_000
        assert_eq!(emergency_limit(&config, "openai", "gpt-4o"), 125_000);
        assert!(is_at_emergency_limit(126_000, &config, "openai", "gpt-4o"));
    }

    /// An explicitly configured window outranks the registry here too.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `if let Some(configured) = self.context_window` early return in
    /// `CompactConfig::effective_context_window`
    /// (wcore-config/src/compact.rs) — the limit jumps to 1_047_000.
    #[test]
    fn explicit_window_outranks_a_known_models_window() {
        let config = CompactConfig {
            context_window: Some(200_000),
            ..default_config()
        };
        assert_eq!(
            emergency_limit(&config, "openai-chatgpt", "gpt-5.4"),
            197_000
        );
        assert!(is_at_emergency_limit(
            198_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
    }

    /// The autocompact threshold must stay strictly BELOW the emergency limit
    /// on a large-window model too — otherwise the fix would move the hard
    /// stop up past the relief valve and the session would die before
    /// compaction ever ran.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: revert either
    /// `emergency_limit` (emergency.rs) or `autocompact_threshold`
    /// (compact/auto.rs) to `config.context_window` — the two boundaries then
    /// sit on different windows and this ordering breaks.
    #[test]
    fn autocompact_still_fires_before_emergency_on_a_large_window_model() {
        use crate::compact::auto::autocompact_threshold;
        let config = default_config();
        let threshold = autocompact_threshold(&config, "openai-chatgpt", "gpt-5.4");
        let limit = emergency_limit(&config, "openai-chatgpt", "gpt-5.4");
        assert!(
            threshold < limit,
            "threshold {threshold} must stay below the hard stop {limit}"
        );
    }

    // ── EMERGENCY_USER_MESSAGE ─────────────────────────────────────────

    #[test]
    fn user_message_mentions_compact() {
        assert!(EMERGENCY_USER_MESSAGE.contains("/compact"));
    }

    #[test]
    fn user_message_mentions_new_conversation() {
        assert!(EMERGENCY_USER_MESSAGE.contains("new conversation"));
    }
}
