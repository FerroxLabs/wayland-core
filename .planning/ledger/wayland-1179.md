---
issue: 1179
repo: FerroxLabs/wayland
kind: defect
title: "Absolute context buffers saturate to zero on a small served window, so a learned window cannot be used to compact"
status: open
last_verified_commit: 31597b00
criteria:
  - id: c1
    text: "input_ceiling() returns a positive value on a small learned or configured window instead of saturating to zero"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::a_4k_window_cannot_be_compacted_into_and_says_so"
    owner: core
    note: "CompactConfig::scaled_reserves with MAX_RESERVE_FRACTION = 0.55 replaces the absolute buffers below a 60,000 crossover. At 4,096 the ceiling goes 0 -> 2,527, so the saturation is gone."
  - id: c2
    text: "The autocompact threshold on a small window sits above core's own baseline turn rather than below it"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::no_configured_window_anywhere_fires_on_the_baseline_turn"
    owner: core
    note: "RE-GRADED 2026-08-30 and the previous note was TRUE OF THE LEARNED PATH ONLY. The issue's acceptance sentence says 'a learned OR CONFIGURED small window', and supports_compaction was consulted at exactly one call site - AgentEngine::narrow_to_served_window, where #1172's learned figure is admitted. An operator's `[compact] context_window` reached effective_context_window and met no gate at all. REPRODUCED at e7144c30a with a new test in the same file: `window 4096: an OPERATOR-CONFIGURED window core has already judged too small to compact in summarized a conversation that had not started - threshold 1844, baseline turn 3118`, and the sweep failed at the first window it tried, `window 1024: ... threshold 462`. Every configured window from 1,024 to 6,928 fired on the baseline turn; in 4,455..6,928 that is strictly WORSE than the MIN_AUTOCOMPACT_WINDOW_FRACTION = 0.70 fallback #1179 replaced (6,000 -> 4,200 then, 2,700 now). FIX: the refusal is a property of the window, not of the route it arrived by, so it moved into CompactConfig::should_autocompact_at and BOTH triggers go through it - compact/auto.rs::should_autocompact and AgentEngine::should_autocompact_now, the latter via a new compaction_window_now so the threshold's VALUE and the trigger's DECISION cannot end up on different windows. Two call sites found, two changed. The threshold VALUE is untouched (the cache ledger and the context gauge still report it) and emergency_limit is untouched. A permanently silent refusal is indistinguishable from compaction being broken, so bootstrap.rs now emits it once on emit_info - the same surface and the same #1130 reasoning as the unknown-window notice it sits next to. EVIDENCE: no_configured_window_anywhere_fires_on_the_baseline_turn sweeps 1,024..=16,384; a_configured_window_too_small_to_work_in_never_fires pins 4,096/5,000/6,000/6,144/6,928 with preconditions asserting each is in the refused band; the_first_workable_configured_window_still_fires_when_it_should is the negative control at 6,929 (3,118 does not fire, 3,119 does, u64::MAX does) so 'never fires' cannot be satisfied by switching autocompact off; a_configured_window_too_small_to_compact_in_is_announced and a_workable_configured_window_is_not_announced_as_too_small cover the notice. All 11 tests in issue_1179_small_window_buffers_test.rs green, all 6 in issue_1150_unknown_context_window_test.rs green, wcore-agent --lib 2,614 passed."
  - id: c3
    text: "Behaviour is measured at the 4k, 8k, 32k, 60k and 200k window points rather than derived from a chosen fraction"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::the_trigger_stays_below_the_ceiling_at_every_window"
    owner: core
    note: "One named test per point - 4k, 8k, 32k, 60k, 200k - each asserting exact pinned (threshold, ceiling) pairs, plus this sweep over 1,024..=262,144 step 512 so the ordering is not merely true at the five samples."
  - id: c4
    text: "A test at each of those points distinguishes compacts usefully from fires every turn"
    state: met
    evidence: "symbol:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::assert_compacts_usefully"
    owner: core
    note: "The P1..P4 predicate applied at every point: threshold > baseline turn, ceiling > baseline turn, threshold < ceiling, ceiling < emergency, plus !should_autocompact(BASELINE_TURN_TOKENS) - literally compacts usefully versus fires every turn. Read through the same functions the engine enforces with."
  - id: c5
    text: "The 33k-110k band does not regress: a pinned 60000 window keeps a threshold below its own pre-flight shed ceiling"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::a_60k_window_is_byte_for_byte_unchanged"
    owner: core
    note: "60,000 asserts threshold 27,000 / ceiling 37,000 / emergency 57,000, the exact pre-#1179 values; 0.55 is the largest fraction whose scale at 60,000 is exactly 1.0. nothing_at_or_above_the_crossover_is_touched asserts the crossover as behaviour rather than arithmetic."
---

#1172 taught core to learn an endpoint's genuinely-served context window from
`usage.prompt_tokens`. That learned figure deliberately does not feed the #255
pre-flight guard or the compaction thresholds, because the buffers are absolute
and were tuned when the only window in play was 200,000. At 4,096 they brick the
run rather than save it: the input ceiling saturates to zero and the autocompact
threshold falls below core's own baseline turn.

So the learned window today drives only the user-facing notice and the pressure
gauge. This issue is the compensation half, and it is the reason #1172's own c3
is not met.

The scope is narrower than it looks. At 4,096 no compaction strategy can work at
all — the honest remedy is the operator raising the server's context length,
which the notice now says. This is about the band where a small window is still
workable, roughly 8k to 32k. c5 is here because the obvious fix has a known way
of making a different band worse, and that band has never been measured.
