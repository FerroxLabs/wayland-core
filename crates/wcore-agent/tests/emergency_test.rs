//! Black-box integration tests for emergency truncation (TC-2.5-01 .. TC-2.5-04).
//!
//! These tests treat `is_at_emergency_limit` as a public API and verify
//! functional requirements from test-plan.md without relying on internal details.

use wcore_agent::compact::emergency::{
    EMERGENCY_USER_MESSAGE, emergency_limit, is_at_emergency_limit,
};
use wcore_config::compact::CompactConfig;

/// A provider/model pair the `wcore_config::limits` registry does NOT know. The
/// TC-2.5-* cases below specify the buffer ARITHMETIC, which is
/// model-independent.
const UNKNOWN_PROVIDER: &str = "test-provider";
const UNKNOWN_MODEL: &str = "test-model";

/// #1150: the window is PINNED. These cases used to get 200,000 by accident,
/// from the unlisted-model fallback; that fallback is now the conservative
/// `UNVERIFIED_CONTEXT_WINDOW`. Without the pin `tc_2_5_02`/`tc_2_5_03` would
/// still pass — 198,000 clears a 29,768 limit trivially — while their comments
/// claimed a 197,000 boundary they were no longer testing.
fn arithmetic_config() -> CompactConfig {
    CompactConfig {
        context_window: Some(200_000),
        ..CompactConfig::default()
    }
}

// ── TC-2.5-01: Below emergency threshold ───────────────────────────────────

#[test]
fn tc_2_5_01_below_emergency_threshold() {
    // context_window=200_000 (fallback), emergency_buffer=3_000
    // emergency_limit = 200k - 3k = 197k
    // 190k < 197k → false
    let config = arithmetic_config();
    assert!(
        !is_at_emergency_limit(
            190_000,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "190k tokens should be below the 197k emergency limit"
    );
}

// ── TC-2.5-02: Above emergency threshold ───────────────────────────────────

#[test]
fn tc_2_5_02_above_emergency_threshold() {
    // 198k >= 197k → true
    let config = arithmetic_config();
    assert!(
        is_at_emergency_limit(
            198_000,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "198k tokens should exceed the 197k emergency limit"
    );
}

// ── TC-2.5-03: Exactly at emergency threshold ──────────────────────────────

#[test]
fn tc_2_5_03_at_exact_emergency_threshold() {
    // 197k >= 197k → true
    let config = arithmetic_config();
    assert!(
        is_at_emergency_limit(
            197_000,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "197k tokens should trigger at exactly the emergency limit"
    );
}

// ── TC-2.5-04: Small context window ────────────────────────────────────────

/// #1179 — the reserve buffers are scaled to the window, so an 8,000-token
/// window's hard stop is 7,600, not 5,000.
///
/// The old 5,000 was unreachable in practice: on the same window the #255
/// pre-flight ceiling was `8_000 - 20_000 - 3_000` saturated to 0, so the guard
/// aborted the run before the hard stop could ever be consulted. What this case
/// is really for — a small window still has a REACHABLE, correctly-ordered hard
/// stop — is asserted below, at the number the engine now enforces.
#[test]
fn tc_2_5_04_small_context_window() {
    let config = CompactConfig {
        context_window: Some(8_000),
        emergency_buffer: 3_000,
        ..CompactConfig::default()
    };
    assert!(
        !is_at_emergency_limit(
            6_000,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "6k is below the scaled 7,600 hard stop on an 8k window"
    );
    assert!(
        is_at_emergency_limit(
            7_600,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "an 8k context window must still have a reachable hard stop"
    );
    assert!(
        config.input_ceiling_for_window(8_000) < 7_600,
        "the hard stop must sit above the pre-flight ceiling it backs up"
    );
}

// ── Additional integration-level checks ────────────────────────────────────

#[test]
fn emergency_check_ignores_enabled_flag() {
    // Emergency is the safety net — it fires even when compact is disabled
    let config = CompactConfig {
        enabled: false,
        ..CompactConfig::default()
    };
    assert!(
        is_at_emergency_limit(
            198_000,
            &config,
            config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
        ),
        "emergency check must fire regardless of the enabled flag"
    );
}

#[test]
fn user_message_is_actionable() {
    // The message should tell the user what to do
    assert!(
        EMERGENCY_USER_MESSAGE.contains("/compact"),
        "emergency message should mention /compact"
    );
    assert!(
        EMERGENCY_USER_MESSAGE.contains("new conversation"),
        "emergency message should mention starting a new conversation"
    );
}

#[test]
fn autocompact_fires_before_emergency() {
    // Verify that the autocompact threshold is lower than the emergency limit
    // so autocompact gets a chance to run before the safety net kicks in.
    use wcore_agent::compact::auto::should_autocompact;

    let config = arithmetic_config();

    // Pick a token count that triggers autocompact but not emergency
    let token_count: u64 = 170_000;
    let autocompact_triggers =
        should_autocompact(token_count, &config, UNKNOWN_PROVIDER, UNKNOWN_MODEL);
    let emergency_triggers = is_at_emergency_limit(
        token_count,
        &config,
        config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL),
    );

    assert!(
        autocompact_triggers && !emergency_triggers,
        "at 170k tokens, autocompact should trigger (threshold 167k) \
         but emergency should not (limit 197k)"
    );
}

#[test]
fn both_trigger_near_limit() {
    // When very close to the limit, both autocompact and emergency should fire
    use wcore_agent::compact::auto::should_autocompact;

    let config = arithmetic_config();
    let token_count: u64 = 198_000;

    assert!(should_autocompact(
        token_count,
        &config,
        UNKNOWN_PROVIDER,
        UNKNOWN_MODEL
    ));
    assert!(is_at_emergency_limit(
        token_count,
        &config,
        config.effective_context_window(UNKNOWN_PROVIDER, UNKNOWN_MODEL)
    ));
}

// ── GH#635: the hard stop is the ACTIVE MODEL's, not a hardcoded 200k ──────

/// A 1.05M-window model must not be emergency-stopped ~850k tokens early.
/// This is the reported symptom: `gpt-5.4` died at ~197k.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: change
/// `config.effective_context_window(provider, model)` back to
/// `config.context_window` in `emergency_limit`
/// (crates/wcore-agent/src/compact/emergency.rs) — the limit collapses to
/// 197_000 and the first assertion fires.
#[test]
fn gh635_large_window_model_is_not_stopped_at_the_200k_default() {
    // #1150: deliberately UNPINNED — this case is about the registry window
    // beating the fallback, and an operator `context_window` outranks the
    // registry.
    let config = CompactConfig::default();
    assert!(
        !is_at_emergency_limit(
            197_000,
            &config,
            config.effective_context_window("openai-chatgpt", "gpt-5.4")
        ),
        "a 197k session on a 1,050,000-token model has ~850k tokens of headroom"
    );
    assert_eq!(
        emergency_limit(
            &config,
            config.effective_context_window("openai-chatgpt", "gpt-5.4")
        ),
        1_047_000,
        "1_050_000 - 3_000"
    );
    // The real boundary still exists and still fires.
    assert!(is_at_emergency_limit(
        1_047_000,
        &config,
        config.effective_context_window("openai-chatgpt", "gpt-5.4")
    ));
}

/// The operator keeps the last word: an explicit `context_window` is honoured
/// over the model's larger registry window.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
/// `if let Some(configured) = self.context_window { return configured; }`
/// early return in `CompactConfig::effective_context_window`
/// (crates/wcore-config/src/compact.rs) — the operator's 200k cap is replaced
/// by 1_050_000 and this assertion fires.
#[test]
fn gh635_explicit_operator_window_is_honoured_over_the_registry() {
    let config = CompactConfig {
        context_window: Some(200_000),
        ..CompactConfig::default()
    };
    assert!(
        is_at_emergency_limit(
            198_000,
            &config,
            config.effective_context_window("openai-chatgpt", "gpt-5.4")
        ),
        "an operator who pinned 200k must still be stopped at 197k"
    );
    assert_eq!(
        emergency_limit(
            &config,
            config.effective_context_window("openai-chatgpt", "gpt-5.4")
        ),
        197_000
    );
}
