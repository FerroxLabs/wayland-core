//! FerroxLabs/wayland#1172 — the served-window detector, replayed against the
//! turns it was calibrated on.
//!
//! Every pair below is MEASURED, not modelled. #1172's reproduction drove a
//! real `qwen3:8b` on stock Ollama (served slot 4,096; `ollama ps` /
//! `n_ctx_slot = 4096`, `n_keep = 4`) through a logging reverse proxy that
//! captured each request body next to the `usage` block it came back with.
//! `sent` is Core's own `char/4` estimate of the captured request; `reported`
//! is the `prompt_tokens` the endpoint returned. Preserved on hetzner at
//! `/root/w3/proxylog{1,2,3,4}/` with `/root/w3/proxy.py`, the instrument.
//!
//! The controls are the point. 24 of these 25 turns were served IN FULL, and
//! a detector that fired on any of them would be worse than no detector: it
//! would clamp a healthy session's window to a number the endpoint never
//! imposed.

use wcore_config::context_window::{ServedWindowTracker, TruncationSignal};

const ROUTE: &str = "openai/qwen3:8b";

/// Replay a `(sent, reported)` sequence and return, per turn, the served
/// window the detector learned (`None` where it stayed silent).
fn replay(turns: &[(u64, u64)]) -> Vec<Option<u64>> {
    let mut tracker = ServedWindowTracker::default();
    turns
        .iter()
        .map(|(sent, reported)| {
            tracker
                .observe(ROUTE, *sent, *reported)
                .map(|e| e.served_window)
        })
        .collect()
}

/// CONTROL — `proxylog1`, six consecutive turns the endpoint served in full.
/// Ratios 0.863..0.874: Core's estimator runs ~15% high on every one of them,
/// and none of that is truncation.
#[test]
fn a_full_service_session_is_never_accused() {
    let observed = replay(&[
        (3550, 3104),
        (3604, 3131),
        (3669, 3179),
        (3731, 3225),
        (3793, 3272),
        (3964, 3442),
    ]);
    assert!(
        observed.iter().all(Option::is_none),
        "every one of these turns was served in full; got {observed:?}"
    );
}

/// CONTROL — `proxylog3` turns 1..11, eleven full-service turns whose ratio
/// band (0.873..0.902) is the top of the measured range.
#[test]
fn the_widest_measured_healthy_band_is_never_accused() {
    let observed = replay(&[
        (3564, 3110),
        (3724, 3316),
        (3798, 3373),
        (3875, 3446),
        (3952, 3519),
        (4029, 3592),
        (4106, 3665),
        (4221, 3780),
        (4298, 3853),
        (4412, 3977),
        (4489, 4050),
    ]);
    assert!(
        observed.iter().all(Option::is_none),
        "these eleven turns all fit the 4,096 slot; got {observed:?}"
    );
}

/// THE gross case — `proxylog4`. Turn 3 sent an estimated 10,466 tokens; the
/// endpoint answered `prompt_tokens: 4095` and its journal logged
/// `truncated = 1`. Ratio 0.391, and 6,371 tokens short.
///
/// The learned figure is 4,095 — the largest prompt this endpoint has been
/// observed to actually process, which is a LOWER BOUND on the slot (the real
/// slot was 4,096) and never an over-estimate.
#[test]
fn a_gross_shortfall_is_detected_and_bounds_the_slot() {
    let mut tracker = ServedWindowTracker::default();
    assert_eq!(tracker.observe(ROUTE, 3583, 3118), None);
    assert_eq!(tracker.observe(ROUTE, 3649, 3165), None);
    let evidence = tracker
        .observe(ROUTE, 10466, 4095)
        .expect("10,466 sent, 4,095 processed - the endpoint discarded ~60% of the prompt");
    assert_eq!(evidence.signal, TruncationSignal::Shortfall);
    assert_eq!(evidence.served_window, 4095);
    assert_eq!(tracker.served_window(), Some(4095));
}

/// THE subtle case — `proxylog3` turn 12. The reported count went DOWN
/// (4,050 → 3,910) while the prompt GREW (4,489 → 4,617 estimated). Prompt
/// tokens are monotone in the prompt for a fixed tokenizer, so this cannot
/// happen unless the server dropped content.
///
/// Its ratio is 0.847 — INSIDE the measured healthy band (0.839..0.902). This
/// turn is exactly why the ratio arm alone is not enough, and why the issue
/// warned that a single-turn "reported < estimated" test sits in estimator
/// noise.
#[test]
fn a_reported_count_that_goes_backwards_is_detected() {
    let mut tracker = ServedWindowTracker::default();
    for pair in [
        (3564u64, 3110u64),
        (3724, 3316),
        (3798, 3373),
        (3875, 3446),
        (3952, 3519),
        (4029, 3592),
        (4106, 3665),
        (4221, 3780),
        (4298, 3853),
        (4412, 3977),
        (4489, 4050),
    ] {
        assert_eq!(tracker.observe(ROUTE, pair.0, pair.1), None, "{pair:?}");
    }
    let evidence = tracker
        .observe(ROUTE, 4617, 3910)
        .expect("the prompt grew by 128 estimated tokens and the endpoint reported 140 FEWER");
    assert_eq!(evidence.signal, TruncationSignal::Regression);
    assert_eq!(
        evidence.served_window, 4050,
        "the ceiling is the largest prompt this endpoint was seen to process"
    );
}

/// A DECLARED LIMIT, held here so it is a decision rather than a surprise.
///
/// `proxylog2` turn 4 WAS truncated — 4,882 estimated sent, `prompt_tokens`
/// 4,095 back, against the same 4,096 slot — and this detector stays silent on
/// it. Its ratio is 0.839, the lowest full-service ratio in the whole corpus,
/// and it is the first turn to overflow, so there is no earlier count for it
/// to regress against. Firing here would mean lowering the ratio arm into the
/// measured healthy band, which would accuse full-service sessions instead.
///
/// The cost is bounded: a session that keeps going regresses or plateaus on
/// the NEXT turn and is caught then. The run in `proxylog2` ended on this turn.
#[test]
fn a_marginal_first_overflow_is_not_yet_distinguishable() {
    let observed = replay(&[(3583, 3118), (3650, 3166), (3738, 3231), (4882, 4095)]);
    assert!(
        observed.iter().all(Option::is_none),
        "documented limit: a first overflow this marginal is inside estimator noise; \
         got {observed:?}"
    );
}

/// A turn the provider reported no usage for is evidence in NEITHER direction.
///
/// It must not manufacture a regression on the following turn - and, the half
/// this test was missing until a mutation run caught it surviving, it must not
/// ERASE the baseline either. A zero recorded as though it were a real count
/// blinds the regression arm to the very next truncated turn, which is the one
/// case the arm exists for.
#[test]
fn an_unreported_turn_neither_fakes_a_regression_nor_erases_the_baseline() {
    // Half one: the zero must not become something to regress FROM.
    let mut tracker = ServedWindowTracker::default();
    assert_eq!(tracker.observe(ROUTE, 4489, 4050), None);
    assert_eq!(
        tracker.observe(ROUTE, 4500, 0),
        None,
        "a zero is not a count"
    );
    assert_eq!(
        tracker.observe(ROUTE, 4617, 4100),
        None,
        "4,100 is MORE than the 4,050 two turns back; a zero in between must not have \
         become the baseline it regressed from"
    );

    // Half two: the same gap, but the turn after it is the MEASURED
    // truncation. The 4,050 baseline has to have survived the unreported turn
    // for the regression arm to still see it.
    let mut tracker = ServedWindowTracker::default();
    assert_eq!(tracker.observe(ROUTE, 4489, 4050), None);
    assert_eq!(tracker.observe(ROUTE, 4500, 0), None);
    let evidence = tracker
        .observe(ROUTE, 4617, 3910)
        .expect("an unreported turn must not blind the detector to the next truncated one");
    assert_eq!(evidence.signal, TruncationSignal::Regression);
    assert_eq!(evidence.served_window, 4050);
}

/// Observations are per-route. A different provider or model tokenizes
/// differently, and the few-percent shift that causes is the same order as the
/// regression arm — comparing across a swap would manufacture evidence.
#[test]
fn a_model_swap_discards_the_history_it_would_otherwise_misread() {
    let mut tracker = ServedWindowTracker::default();
    assert_eq!(tracker.observe(ROUTE, 4489, 4050), None);
    assert_eq!(
        tracker.observe("openai/other-model", 4617, 3910),
        None,
        "3,910 < 4,050 only because the previous turn was a DIFFERENT tokenizer"
    );
    assert_eq!(tracker.served_window(), None);
}

/// The user is told once per figure, not once per turn: a session that keeps
/// truncating at the same ceiling must not repeat the notice every turn.
#[test]
fn a_stable_ceiling_is_reported_once() {
    let mut tracker = ServedWindowTracker::default();
    assert!(tracker.observe(ROUTE, 10466, 4095).is_some());
    assert_eq!(
        tracker.observe(ROUTE, 11000, 4095),
        None,
        "same ceiling, already announced"
    );
    assert_eq!(tracker.served_window(), Some(4095));
}

// ————————— FerroxLabs/wayland-core#353: corroboration before the trigger moves ————————

/// The TELLING half. One qualifying turn still produces evidence and still
/// establishes `served_window()` — the notice keeps its one-observation
/// sensitivity, which #353 explicitly forbids regressing.
///
/// The SIZING half does not. `sizing_window()` is what
/// `wcore_agent::Engine::narrow_to_served_window` reads, and it stays `None`
/// until the verdict is corroborated.
#[test]
fn a_single_regression_tells_the_user_but_does_not_yet_size_the_session() {
    let mut tracker = ServedWindowTracker::default();
    assert_eq!(
        tracker.observe(ROUTE, 4489, 4050),
        None,
        "the baseline turn"
    );

    let evidence = tracker
        .observe(ROUTE, 4617, 3910)
        .expect("the NOTICE must still fire on one observation");
    assert_eq!(evidence.signal, TruncationSignal::Regression);
    assert_eq!(evidence.served_window, 4050);
    assert_eq!(
        tracker.served_window(),
        Some(4050),
        "the telling figure is available immediately"
    );

    assert_eq!(
        tracker.sizing_window(),
        None,
        "one anomalous usage report must not be enough to compact the user\'s \
         conversation (#353)"
    );
}

/// The other half, and the reason #353 is not closed by disabling the tracker:
/// on a real truncating endpoint the regression repeats on the very next turn,
/// so corroboration costs exactly one turn.
#[test]
fn a_second_regression_corroborates_it_and_the_session_is_sized() {
    let mut tracker = ServedWindowTracker::default();
    tracker.observe(ROUTE, 4489, 4050);
    tracker.observe(ROUTE, 4617, 3910);
    assert_eq!(tracker.sizing_window(), None, "still one observation");

    // The second regression sits at an UNCHANGED ceiling, and it is the turn
    // that flips this route from telling to SIZING. wayland-core#353 D10: it
    // used to be swallowed by the once-per-figure suppression, which made the
    // notice's sizing sentence unfalsifiable — emitted on the first
    // observation, when nothing had moved, and never again, when it had. A
    // state change the user must be told about re-opens the gate exactly once.
    let corroborating = tracker
        .observe(ROUTE, 4700, 3800)
        .expect("corroboration is a state change, not a repeat");
    assert!(
        corroborating.corroborated,
        "and the evidence must SAY so, or the caller cannot pick its wording"
    );
    assert_eq!(corroborating.served_window, 4050);
    // Exactly once: a third regression at the same figure changes nothing.
    assert_eq!(
        tracker.observe(ROUTE, 4800, 3700),
        None,
        "the ceiling has not moved and corroboration already landed"
    );
    assert_eq!(
        tracker.sizing_window(),
        Some(4050),
        "a repeated regression must still size the session, or the fix is just \
         a disabled detector"
    );
    assert_eq!(
        tracker.served_window(),
        tracker.sizing_window(),
        "once corroborated the two figures agree"
    );
}

/// A `Shortfall` corroborates itself, so #1172\'s measured behaviour is
/// unchanged. Its arm already requires a miss of at least
/// `MIN_SHORTFALL_TOKENS` AND a ratio below `SERVED_SHORTFALL_RATIO`, which is
/// the absolute magnitude the `Regression` arm has no equivalent of. Without
/// this the fix would have delayed every real detection by a turn.
#[test]
fn a_shortfall_carries_its_own_corroboration() {
    let mut tracker = ServedWindowTracker::default();
    let evidence = tracker
        .observe(ROUTE, 10_466, 4_095)
        .expect("the gross shortfall #1172 measured against a stock Ollama");
    assert_eq!(evidence.signal, TruncationSignal::Shortfall);
    assert_eq!(
        tracker.sizing_window(),
        Some(4_095),
        "a shortfall is corroborated within its own turn"
    );
}

/// A `Shortfall` is also the "agreement with a second signal" that corroborates
/// an earlier `Regression`, which is why the rule is stated as evidence rather
/// than as a regression counter alone.
#[test]
fn a_shortfall_corroborates_an_earlier_regression() {
    let mut tracker = ServedWindowTracker::default();
    tracker.observe(ROUTE, 4489, 4050);
    tracker.observe(ROUTE, 4617, 3910);
    assert_eq!(tracker.sizing_window(), None);

    tracker.observe(ROUTE, 10_000, 3_500);
    assert_eq!(
        tracker.sizing_window(),
        Some(4050),
        "a second, independent signal on the same route corroborates the first"
    );
}

/// A model swap throws the corroboration away with everything else. A different
/// tokenizer shifts the reported count by the same order as the `Regression`
/// arm, so carrying evidence across the swap would manufacture it.
#[test]
fn a_model_swap_discards_the_corroboration_too() {
    let mut tracker = ServedWindowTracker::default();
    tracker.observe(ROUTE, 4489, 4050);
    tracker.observe(ROUTE, 4617, 3910);
    tracker.observe(ROUTE, 4700, 3800);
    assert_eq!(tracker.sizing_window(), Some(4050), "corroborated here");

    tracker.observe("openai/qwen3:14b", 4800, 4700);
    assert_eq!(
        tracker.sizing_window(),
        None,
        "the new route has no evidence of its own"
    );
}
