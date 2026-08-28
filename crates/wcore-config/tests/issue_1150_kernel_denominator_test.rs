//! #1150 — what the compaction config hands the `ContextWindow` kernel.
//!
//! `ContextWindow::resolve`'s contract is explicit: "when both are absent the
//! window is `None` and no denominator is fabricated". Production never let it
//! honour that, because every call site passed
//! `CompactConfig::fallback_context_window()`, which is `context_window` OR a
//! flat 200,000 — never zero. `resolve` therefore returned `Some(200_000)` for
//! every unlisted model and the `None` arm of the kernel was reachable only
//! from the kernel's own unit tests.
//!
//! These tests pin the ARGUMENT, not the kernel: the kernel was always right.

use wcore_config::compact::CompactConfig;
use wcore_config::context_window::ContextWindow;

/// The reporter's model: a 32k local model served over an OpenAI-compatible
/// endpoint, matched by no arm of `limits::model_output_ceiling` and by no
/// Flux tier alias.
const UNLISTED: &str = "issue-1150-local-32k-unlisted";

/// The watermark the reporter was sitting at when they filed.
const REPORTED_TOKENS: u64 = 83_208;

#[test]
fn an_unlisted_model_gets_no_fabricated_kernel_denominator() {
    let cfg = CompactConfig::default();
    let ctx = ContextWindow::resolve(
        REPORTED_TOKENS,
        "openai",
        UNLISTED,
        cfg.kernel_config_window(),
    );
    assert_eq!(
        ctx.window, None,
        "the kernel promises `None` for an unknown model with no operator override, but the \
         config handed it a fabricated denominator"
    );
    assert_eq!(
        ctx.percent(),
        None,
        "a `% full` gauge computed against a guessed window is a lie the user acts on"
    );
    assert_eq!(
        ctx.input_ceiling(cfg.output_reserve as u64, cfg.emergency_buffer as u64),
        None,
        "the pre-flight shed/overflow ceiling must fail open on an unknown window rather \
         than fire at 200_000 - 20_000 - 3_000 = 177_000 tokens on a 32k model"
    );
}

/// The operator's explicit setting is the ONE fallback the kernel may use, and
/// it must still reach it.
#[test]
fn an_operator_override_is_still_handed_to_the_kernel() {
    let cfg = CompactConfig {
        context_window: Some(32_768),
        ..CompactConfig::default()
    };
    assert_eq!(cfg.kernel_config_window(), 32_768);
    let ctx = ContextWindow::resolve(
        REPORTED_TOKENS,
        "openai",
        UNLISTED,
        cfg.kernel_config_window(),
    );
    assert_eq!(ctx.window, Some(32_768));
    // 83,208 against a real 32,768 window is 254% full, and the pre-flight
    // ceiling (32_768 - 20_000 - 3_000 = 9_768) is long since crossed — the
    // guard the reporter never got.
    assert_eq!(ctx.percent(), Some(254));
    let ceiling = ctx
        .input_ceiling(cfg.output_reserve as u64, cfg.emergency_buffer as u64)
        .expect("a known window yields a ceiling");
    assert_eq!(ceiling, 9_768);
    assert!(ctx.used_tokens >= ceiling);
}

/// A model the registry DOES know is unaffected: the kernel reads its real
/// window and never consults the config argument at all.
#[test]
fn a_known_model_is_unaffected_by_the_missing_fallback() {
    let cfg = CompactConfig::default();
    let ctx = ContextWindow::resolve(64_000, "openai", "gpt-4o", cfg.kernel_config_window());
    assert_eq!(ctx.window, Some(128_000));
    assert_eq!(ctx.percent(), Some(50));
}

/// A Flux tier alias keeps its conservative pool-minimum floor (CORE-4) with
/// no config fallback in play — dropping the fabricated 200k must not reopen
/// the wedge that floor was added to close.
#[test]
fn a_flux_tier_alias_keeps_its_conservative_floor() {
    let cfg = CompactConfig::default();
    for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
        let ctx = ContextWindow::resolve(96_000, "openai", alias, cfg.kernel_config_window());
        assert_eq!(
            ctx.window,
            Some(128_000),
            "{alias} must still resolve the 128k floor"
        );
    }
}

// -- the honest window lookup -----------------------------------------------

#[test]
fn known_context_window_is_none_for_an_unlisted_model() {
    let cfg = CompactConfig::default();
    assert_eq!(cfg.known_context_window("openai", UNLISTED), None);
}

#[test]
fn known_context_window_reads_the_registry_for_a_known_model() {
    let cfg = CompactConfig::default();
    assert_eq!(
        cfg.known_context_window("openai", "gpt-4o"),
        Some(128_000),
        "gpt-4o's real window, not the 200k fallback"
    );
}

#[test]
fn known_context_window_prefers_the_operator_setting() {
    let cfg = CompactConfig {
        context_window: Some(32_768),
        ..CompactConfig::default()
    };
    assert_eq!(
        cfg.known_context_window("openai", "gpt-4o"),
        Some(32_768),
        "an explicit operator cap must not be silently replaced by the registry number"
    );
}

/// `effective_context_window` keeps its documented 200,000 FALLBACK for the
/// static autocompact/emergency thresholds — it returns `usize`, promises no
/// `None`, and the ledger/protocol surface reports it as a plain integer. This
/// pins that the delegation refactor did not change any of those numbers.
#[test]
fn effective_context_window_keeps_its_declared_fallback() {
    let cfg = CompactConfig::default();
    assert_eq!(cfg.effective_context_window("openai", UNLISTED), 200_000);
    assert_eq!(cfg.effective_context_window("openai", "gpt-4o"), 128_000);
    assert_eq!(
        cfg.effective_context_window("openai", "flux-auto"),
        128_000,
        "the router-alias floor may only ever lower the boundary"
    );
    let pinned = CompactConfig {
        context_window: Some(50_000),
        ..CompactConfig::default()
    };
    assert_eq!(pinned.effective_context_window("openai", "gpt-4o"), 50_000);
}
