//! FerroxLabs/wayland#1179 — the compaction boundaries, measured at every
//! window size the product can meet.
//!
//! #1172 gave core the ability to LEARN an endpoint's genuinely-served context
//! window from `usage.prompt_tokens`. That figure could not be pointed at the
//! #255 pre-flight guard or at compaction, because the reserve buffers were
//! ABSOLUTE — tuned when the only window in play was 200,000 — and at a served
//! window of 4,096 they consumed 806% of it. Feeding it in naively bricked the
//! run: `input_ceiling()` saturated to 0 (the guard fires on every turn and
//! aborts) and the autocompact threshold landed below core's own baseline turn
//! (an LLM summarization at the top of every turn, forever).
//!
//! This file is the measurement #1179 asks for: the five window points, and at
//! each one a test that distinguishes **compacts usefully** from **fires every
//! turn**.
//!
//! # The four properties, stated once
//!
//! For a window `w`, with `B` = [`BASELINE_TURN_TOKENS`] (3,118 — core's own
//! system prompt plus eight tool schemas, measured in #1172 off a real
//! endpoint's `usage` block, before the user has said anything):
//!
//! * **P1 `threshold > B`** — compaction does not fire on an empty
//!   conversation. Violated ⇒ *fires every turn*.
//! * **P2 `ceiling > B`** — the pre-flight guard does not abort before the user
//!   has typed anything. Violated ⇒ *aborts every run*.
//! * **P3 `threshold < ceiling`** — compaction gets its chance BEFORE the guard
//!   sheds and aborts. Violated ⇒ the guard fires first and compaction, sitting
//!   above it, can never run at all. This is what was happening at 32,768: the
//!   threshold was 22,937 and the ceiling 9,768.
//! * **P4 `ceiling < emergency_limit`** — the hard stop is last.
//!
//! # What is deliberately NOT asserted
//!
//! That 4,096 works. It does not, and no arithmetic makes it: core's baseline
//! turn alone is 3,118 of those 4,096 tokens — 76% of the window before any
//! work happens. [`a_4k_window_cannot_be_compacted_into_and_says_so`] pins that
//! as a measured finding rather than leaving it as a TODO. The remedy there is
//! the operator raising the server's context length, which is what #1172's
//! notice already tells them, and `CompactConfig::supports_compaction` is what
//! stops core narrowing its own guard onto a window it cannot work in.

use wcore_agent::compact::auto::{autocompact_threshold, should_autocompact};
use wcore_agent::compact::emergency::emergency_limit;
use wcore_config::compact::{BASELINE_TURN_TOKENS, CompactConfig};
use wcore_config::context_window::ContextWindow;

/// A model no catalogue knows, so `effective_context_window` takes the operator
/// override below and nothing else.
const UNKNOWN_PROVIDER: &str = "some-self-hosted-thing";
const UNKNOWN_MODEL: &str = "a-model-no-catalogue-knows";

fn pinned(window: usize) -> CompactConfig {
    CompactConfig {
        context_window: Some(window),
        ..CompactConfig::default()
    }
}

/// `(threshold, ceiling, emergency_limit)` at `window`, all three read through
/// the SAME functions the engine enforces with.
fn boundaries(window: usize) -> (usize, usize, usize) {
    let cfg = pinned(window);
    let threshold = autocompact_threshold(&cfg, UNKNOWN_PROVIDER, UNKNOWN_MODEL);
    let ceiling = ContextWindow {
        used_tokens: 0,
        window: Some(window as u64),
    }
    .input_ceiling(&cfg)
    .expect("a pinned window always yields a ceiling") as usize;
    let emergency = emergency_limit(&cfg, UNKNOWN_PROVIDER, UNKNOWN_MODEL);
    (threshold, ceiling, emergency)
}

/// P1..P4 at one window. Returns nothing; panics with the property that broke.
fn assert_compacts_usefully(window: usize) {
    let (threshold, ceiling, emergency) = boundaries(window);
    let cfg = pinned(window);
    assert!(
        threshold > BASELINE_TURN_TOKENS,
        "P1 window {window}: threshold {threshold} is at or below the {BASELINE_TURN_TOKENS}-token \
         baseline turn, so compaction fires on an empty conversation, every turn, forever"
    );
    assert!(
        ceiling > BASELINE_TURN_TOKENS,
        "P2 window {window}: ceiling {ceiling} is at or below the {BASELINE_TURN_TOKENS}-token \
         baseline turn, so the #255 guard aborts the run before the user has typed anything"
    );
    assert!(
        threshold < ceiling,
        "P3 window {window}: threshold {threshold} sits ABOVE the pre-flight ceiling {ceiling}, \
         so the guard sheds and aborts first and compaction can never fire"
    );
    assert!(
        ceiling < emergency,
        "P4 window {window}: ceiling {ceiling} is at or above the emergency limit {emergency}"
    );
    assert!(
        !should_autocompact(
            BASELINE_TURN_TOKENS as u64,
            &cfg,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ),
        "window {window}: a baseline turn with no user content triggered a summarization"
    );
    assert!(
        cfg.supports_compaction(window),
        "window {window}: P1..P4 all hold but `supports_compaction` says no, so the learned \
         window would never be pointed at boundaries that work"
    );
}

// ── The five points #1179 names ─────────────────────────────────────────────

/// **4,096 — the slot #1172 measured a stock Ollama actually serving.**
///
/// The honest finding, with the numbers, rather than an open TODO: no
/// compaction strategy works here, and the arithmetic says why. Scaling the
/// reserves does lift the ceiling off zero — 0 → 2,527, so the saturation
/// really is gone — but core's own baseline turn is 3,118 tokens, which is
/// larger than the whole input budget. Every boundary is below the cost of
/// existing.
///
/// This is asserted as a REFUSAL, not as a defect to fix later: the product's
/// answer at 4,096 is #1172's notice telling the operator to raise the
/// server's context length, and `supports_compaction` returning false is what
/// keeps core from narrowing its own guard onto a window it cannot serve.
#[test]
fn a_4k_window_cannot_be_compacted_into_and_says_so() {
    let (threshold, ceiling, _) = boundaries(4_096);
    assert_eq!((threshold, ceiling), (1_844, 2_527));
    assert!(
        ceiling > 0,
        "the absolute reserves saturated this to zero, which is what made the \
         #255 guard fire on every turn; scaling them must at least lift it off zero"
    );
    assert!(
        threshold < BASELINE_TURN_TOKENS,
        "if this ever clears the baseline turn, 4,096 has become workable and \
         `supports_compaction` should be allowed to say so"
    );
    assert!(
        !pinned(4_096).supports_compaction(4_096),
        "core must refuse to point its guard at a window its own baseline turn \
         does not fit in — narrowing onto it aborts every run instead of saving any"
    );
}

/// **8,192 — the bottom of the band #1179 calls workable.** It is workable, and
/// only just: 3,688 − 3,118 = 570 tokens of room above the baseline turn.
#[test]
fn an_8k_window_compacts_usefully() {
    assert_compacts_usefully(8_192);
    let (threshold, ceiling, _) = boundaries(8_192);
    assert_eq!((threshold, ceiling), (3_688, 5_053));
}

/// **32,768 — `UNVERIFIED_CONTEXT_WINDOW`, the window #1150 made every unknown
/// model take.** The inversion this fixes is the whole of #1179's "one step
/// further down": before, the threshold was 22,937 and the pre-flight ceiling
/// 9,768, so the guard shed and aborted 13,169 tokens BELOW the trigger and
/// autocompact could never fire — on the default every unlisted model gets.
#[test]
fn a_32k_window_no_longer_inverts_the_guard_and_the_trigger() {
    assert_compacts_usefully(32_768);
    let (threshold, ceiling, _) = boundaries(32_768);
    assert_eq!((threshold, ceiling), (14_747, 20_208));
    // The pre-#1179 pair, stated so the regression is legible.
    let unscaled_ceiling = 32_768usize - 20_000 - 3_000;
    assert_eq!(unscaled_ceiling, 9_768);
    assert!(
        22_937 > unscaled_ceiling,
        "control: the old threshold really did sit above the old ceiling"
    );
}

/// **60,000 — the pinned window #1150's notes name as the thing not to
/// disturb.** A naive proportional FLOOR on the threshold alone would have
/// moved it 27,000 → 42,000, past this window's own pre-flight shed ceiling of
/// 37,000. It must be byte-for-byte unchanged, and it is: 0.55 is the largest
/// reserve fraction for which the scale here is exactly 1.0.
#[test]
fn a_60k_window_is_byte_for_byte_unchanged() {
    let (threshold, ceiling, emergency) = boundaries(60_000);
    assert_eq!(threshold, 27_000, "60_000 - 20_000 - 13_000");
    assert_eq!(ceiling, 37_000, "60_000 - 20_000 - 3_000");
    assert_eq!(emergency, 57_000, "60_000 - 3_000");
    assert_compacts_usefully(60_000);
}

/// **200,000 — the window every absolute buffer was tuned for.** Unchanged.
#[test]
fn a_200k_window_is_byte_for_byte_unchanged() {
    let (threshold, ceiling, emergency) = boundaries(200_000);
    assert_eq!(threshold, 167_000);
    assert_eq!(ceiling, 177_000);
    assert_eq!(emergency, 197_000);
    assert_compacts_usefully(200_000);
}

// ── The properties, everywhere ──────────────────────────────────────────────

/// P3 is the property the absolute buffers could not hold, and it must hold at
/// EVERY window, not just the five sampled points — an ordering that is true at
/// 32,768 and 60,000 but false at 41,000 is not an ordering.
#[test]
fn the_trigger_stays_below_the_ceiling_at_every_window() {
    for window in (1_024..=262_144).step_by(512) {
        let (threshold, ceiling, emergency) = boundaries(window);
        assert!(
            threshold < ceiling,
            "window {window}: threshold {threshold} >= ceiling {ceiling}"
        );
        assert!(
            ceiling < emergency,
            "window {window}: ceiling {ceiling} >= emergency {emergency}"
        );
    }
}

/// The crossover is a claim about WHERE behaviour changes, so it is asserted
/// rather than described: every window at or above 60,000 keeps the raw
/// absolute reserves, and every window below it is scaled. The smallest window
/// in the `limits` catalogue is 128,000, so no catalogued model moves.
#[test]
fn nothing_at_or_above_the_crossover_is_touched() {
    let cfg = CompactConfig::default();
    // The crossover is asserted as BEHAVIOUR, not as `33_000 / 0.55`. That
    // division is 59_999.999... in f64 and truncates to 59_999, while the
    // predicate the code actually evaluates (`33_000.0 <= window * 0.55`) is
    // true at exactly 60_000. Asserting the quotient would have pinned an
    // artefact of the rounding rather than the boundary.
    for window in [60_000usize, 64_000, 128_000, 200_000, 1_050_000] {
        let r = cfg.scaled_reserves(window);
        assert_eq!(r.output_reserve, cfg.output_reserve, "window {window}");
        assert_eq!(
            r.autocompact_buffer, cfg.autocompact_buffer,
            "window {window}"
        );
        assert_eq!(r.emergency_buffer, cfg.emergency_buffer, "window {window}");
    }
    // And just below it, they are not.
    let r = cfg.scaled_reserves(59_000);
    assert!(
        r.output_reserve < cfg.output_reserve,
        "the scale must engage below the crossover, or small windows keep the \
         absolute reserves that saturate them"
    );
}

/// `supports_compaction` is the gate on feeding #1172's learned window into the
/// guard, so its boundary is load-bearing: one token either side of it decides
/// whether core narrows onto a window or leaves it alone.
#[test]
fn supports_compaction_switches_where_the_baseline_turn_stops_fitting() {
    let cfg = CompactConfig::default();
    let mut first_supported = None;
    for window in 1_024usize..16_384 {
        if cfg.supports_compaction(window) {
            first_supported = Some(window);
            break;
        }
    }
    let w = first_supported.expect("some window in 1k..16k must be workable");
    assert!(
        !cfg.supports_compaction(w - 1),
        "the predicate must be monotone at its boundary"
    );
    // 3,118 / (1 - 0.55) = 6,928.9 -> 6,929 is the first window whose threshold
    // clears the baseline turn (3,119 > 3,118); at 6,928 it is exactly 3,118
    // and the predicate refuses. Derived, not chosen.
    assert_eq!(w, 6_929, "the boundary is derived, not chosen");
    assert_eq!(boundaries(6_929).0, 3_119);
    assert_eq!(boundaries(6_928).0, 3_118);
    assert!(!cfg.supports_compaction(4_096));
    assert!(cfg.supports_compaction(8_192));
}

// ── The CONFIGURED path (#1179 c2) ──────────────────────────────────────────

/// #1179's acceptance sentence is "a learned **or configured** small window",
/// and the configured half is the one an operator reaches on purpose: #1150's
/// own notice tells them to set `[compact] context_window`, and a local Ollama
/// `num_ctx` of 6,144 lands squarely inside this band.
///
/// The refusal must therefore be a property of the WINDOW, not of the route the
/// window arrived by. It shipped inside `AgentEngine::narrow_to_served_window`
/// — the one place #1172's LEARNED figure is admitted — which left the
/// configured path unguarded: a `[compact] context_window = 6000` kept a
/// 2,700-token threshold against core's own 3,118-token baseline turn, so
/// `should_autocompact` was already true before the user had typed anything and
/// stayed true after the summary came back, because a system prompt is not
/// something a summarizer can reclaim.
///
/// Measured on the shipped arithmetic, every one of these windows has a
/// threshold at or below the baseline turn, so every one of them is the
/// "fires every turn" side of the c4 distinction.
#[test]
fn a_configured_window_too_small_to_work_in_never_fires() {
    for window in [4_096usize, 5_000, 6_000, 6_144, 6_928] {
        let cfg = pinned(window);
        // Preconditions, asserted so nothing below can pass vacuously: this
        // window really is in the refused band, and its threshold really is
        // under the baseline turn.
        assert!(
            !cfg.supports_compaction(window),
            "window {window} is not in the refused band, so this arm measures nothing"
        );
        let (threshold, _, _) = boundaries(window);
        assert!(
            threshold <= BASELINE_TURN_TOKENS,
            "window {window}: threshold {threshold} already clears the \
             {BASELINE_TURN_TOKENS}-token baseline turn, so this arm measures nothing"
        );

        assert!(
            !should_autocompact(
                BASELINE_TURN_TOKENS as u64,
                &cfg,
                UNKNOWN_PROVIDER,
                UNKNOWN_MODEL
            ),
            "window {window}: an OPERATOR-CONFIGURED window core has already judged too \
             small to compact in summarized a conversation that had not started - \
             threshold {threshold}, baseline turn {BASELINE_TURN_TOKENS}"
        );

        // ...and the refusal is not a race the watermark can win. A threshold
        // below the cost of existing is not a trigger, it is a loop, so no
        // amount of pressure may turn it back on. The emergency hard stop is
        // the boundary that still applies here, and it is untouched.
        assert!(
            !should_autocompact(u64::MAX, &cfg, UNKNOWN_PROVIDER, UNKNOWN_MODEL),
            "window {window}: the refusal evaporated once the watermark grew"
        );
    }
}

/// The negative control for the arm above, at the first window that IS
/// workable. Without it, "never fires" would also be satisfied by a change that
/// simply switched autocompact off.
#[test]
fn the_first_workable_configured_window_still_fires_when_it_should() {
    let cfg = pinned(6_929);
    assert!(cfg.supports_compaction(6_929));
    assert!(
        !should_autocompact(
            BASELINE_TURN_TOKENS as u64,
            &cfg,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ),
        "3,118 is below the 3,119 threshold at 6,929 - by one token, which is the point"
    );
    assert!(
        should_autocompact(3_119, &cfg, UNKNOWN_PROVIDER, UNKNOWN_MODEL),
        "a workable configured window must still compact when the watermark reaches \
         its threshold, or the refusal has eaten the feature"
    );
    assert!(
        should_autocompact(u64::MAX, &cfg, UNKNOWN_PROVIDER, UNKNOWN_MODEL),
        "a workable configured window under real pressure must compact"
    );
}

/// The band the refusal is measured over, stated as a range rather than as five
/// samples, and paired with its complement so the boundary itself is pinned.
///
/// This is also the regression #1179 introduced and c2 is about: below 6,929 a
/// configured window fires on the baseline turn, and between 4,455 and 6,928 it
/// did NOT before #1179 (the `MIN_AUTOCOMPACT_WINDOW_FRACTION = 0.70` fallback
/// gave 4,200 at a 6,000 window). The band that went backwards is inside the
/// sweep below.
#[test]
fn no_configured_window_anywhere_fires_on_the_baseline_turn() {
    for window in 1_024usize..=16_384 {
        let cfg = pinned(window);
        let fires = should_autocompact(
            BASELINE_TURN_TOKENS as u64,
            &cfg,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL,
        );
        assert!(
            !fires,
            "window {window}: a configured window fires an LLM summarization on core's \
             own {BASELINE_TURN_TOKENS}-token baseline turn, before the user has typed \
             anything - threshold {}",
            boundaries(window).0
        );
    }
}
