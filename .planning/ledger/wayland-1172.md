---
issue: 1172
repo: FerroxLabs/wayland
title: "Core cannot see a self-hosted endpoint's served context window: stock Ollama silently discards the system prompt while core reports 6% pressure"
status: open
last_verified_commit: 43848f75
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
    evidence: "file:crates/wcore-agent/src/engine.rs:15348"
    owner: core
    note: "Upgraded from the corpus test, which evidences DETECTION rather than the user-facing notice this criterion is about. engine.rs:15348-15351 is the emit_info site that names the shortfall and says the HEAD of the prompt is what was lost. SOFT SPOT: no test asserts the notice STRING - grep for the phrase returns the production site only."
  - id: c3
    text: "COMPENSATION: the learned window feeds the pre-flight guard and autocompact, so the truncation stops"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_learned_served_window_narrows_the_preflight_window_when_it_is_workable"
    owner: core
    note: "eb2f2635 added narrow_to_served_window / resolve_preflight_window / autocompact_threshold_now; both #255 guard call sites and the autocompact trigger route through them, so the guard, the trigger and the reported threshold cannot disagree. Narrowing is gated on CompactConfig::supports_compaction, so a 4,096 window is deliberately not narrowed onto. The spurious-compaction risk this wiring created is wayland-core#353."
---

Detection shipped in v0.13.10. Compensation did not — and this ticket's BODY
sets that bar, not its title.

The reporter's own words: "32,768 is still 8x the served 4,096 slot, so the
truncation persists". Core now knows the real window and says so, which turns
a silent wrong answer into a visible one. The prompt is still truncated.

c3 is blocked on a real ordering problem, not on effort: wiring the learned
window into the guard today makes a small-window run fail outright, because
the fixed buffers saturate to zero at 4,096. #1179 is that work.
