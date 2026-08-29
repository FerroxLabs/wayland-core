---
issue: 1179
repo: FerroxLabs/wayland
kind: defect
title: "Absolute context buffers saturate to zero on a small served window, so a learned window cannot be used to compact"
status: open
last_verified_commit: 43848f75
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
    evidence: "test:crates/wcore-agent/tests/issue_1179_small_window_buffers_test.rs::an_8k_window_compacts_usefully"
    owner: core
    note: "At 8,192 threshold=3,688 vs BASELINE_TURN_TOKENS=3,118. At 4,096 the threshold is still below the baseline turn and that is handled by REFUSAL, not pretence: supports_compaction(4_096) is false, so core never narrows its guard onto it."
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
