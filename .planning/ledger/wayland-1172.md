---
issue: 1172
repo: FerroxLabs/wayland
kind: defect
title: "Core cannot see a self-hosted endpoint's served context window: stock Ollama silently discards the system prompt while core reports 6% pressure"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "Core learns the window an endpoint actually serves, from the token counts already in its responses"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "no probing and no address sniffing — the signal was already in the bytes we receive"
  - id: c2
    text: "The shortfall is named to the user, and says the HEAD of the prompt is what was lost"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:15926:Servers drop the HEAD of the conversation first"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was engine.rs:15348, drifted onto a FluxRouter web_search sources comment. The notice is emitted at :15761 and the sentence this criterion is about -- that the HEAD of the prompt is what was lost -- is at :15764. Upgraded from the corpus test, which evidences DETECTION rather than the user-facing notice this criterion is about. engine.rs:15348-15351 is the emit_info site that names the shortfall and says the HEAD of the prompt is what was lost. SOFT SPOT: no test asserts the notice STRING - grep for the phrase returns the production site only."
  - id: c3
    text: "COMPENSATION: the learned window feeds the pre-flight guard and autocompact, so the truncation stops"
    state: not-met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_learned_served_window_narrows_the_preflight_window_when_it_is_workable"
    owner: core
    note: "eb2f2635 added narrow_to_served_window / resolve_preflight_window / autocompact_threshold_now; both #255 guard call sites and the autocompact trigger route through them, so the guard, the trigger and the reported threshold cannot disagree. Narrowing is gated on CompactConfig::supports_compaction, so a 4,096 window is deliberately not narrowed onto. The spurious-compaction risk this wiring created is wayland-core#353. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: DOES NOT HOLD AS WRITTEN, and this is the substituted-property failure the brief warns about. The criterion is two clauses. Clause 1 -- 'the learned window feeds the pre-flight guard and autocompact' -- is TRUE and I verified the chokepointing: the #255 guard (engine.rs:13243) and the length-finish check (engine.rs:15089) both call resolve_preflight_window, and should_autocompact_now (engine.rs:18157) calls autocompact_threshold_now; all three route through narrow_to_served_window (engine.rs:8462). The named test passed. Clause 2 -- 'so the truncation stops' -- is FALSE in the exact configuration this ticket reports. narrow_to_served_window returns the window UNCHANGED unless CompactConfig::supports_compaction(served) (compact.rs:608). With MAX_RESERVE_FRACTION=0.55 and BASELINE_TURN_TOKENS=3,118, at the 4,096 slot #1172 measured the scaled ceiling is 2,527 and the threshold 1,844 -- both below 3,118 -- so supports_compaction(4_096) is false and the learned 4,095 is deliberately NOT narrowed onto. Core keeps sizing against UNVERIFIED_CONTEXT_WINDOW=32,768, still sends ~10.5k tokens, and Ollama still truncates. That is verbatim the sentence the ticket used to refuse #1150's close: '32,768 is still 8x the served 4,096 slot, so the truncation persists'. The lane's own test proves the easier property -- a_learned_served_window_narrows_the_preflight_window_when_it_is_workable uses 8,192, not 4,096 -- and its sibling a_learned_served_window_too_small_to_work_in_is_not_narrowed_onto asserts the 4,096 case is skipped. The graded property became 'narrows where workable (8k-32k)'; the ticket asked for the truncation to stop on stock Ollama."
---

Detection shipped in v0.13.10. Compensation did not — and this ticket's BODY
sets that bar, not its title.

The reporter's own words: "32,768 is still 8x the served 4,096 slot, so the
truncation persists". Core now knows the real window and says so, which turns
a silent wrong answer into a visible one. The prompt is still truncated.

c3 is blocked on a real ordering problem, not on effort: wiring the learned
window into the guard today makes a small-window run fail outright, because
the fixed buffers saturate to zero at 4,096. #1179 is that work.
