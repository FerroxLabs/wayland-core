---
issue: 1218
repo: FerroxLabs/wayland
kind: defect
title: "The scaled output_reserve is decoupled from the max_tokens core actually sends, so the pre-flight ceiling admits a request that cannot fit"
status: open
last_verified_commit: 115cb4c6
criteria:
  - id: c1
    text: "size_output_cap and scaled_reserves agree: the max_tokens sent is never larger than the reserve the ceiling withheld"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::size_output_cap"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37, found while verifying FerroxLabs/wayland#1179). Nothing has been done. The measured finding, verbatim: Scaling `output_reserve` down decoupled it from the `max_tokens` core actually asks for, so the pre-flight ceiling now certifies inputs that cannot fit alongside the output request. The scaled reserve is exactly w/3 (20,000 x 0.55w/33,000), while `size_output_cap` sizes `max_tokens` from the CATALOGUED window (or `UNKNOWN_CAP` = 8,192 for an unlisted model) and never sees the compact window or #1172's learned served window. Concretely, on the endpoint #1172 measured (unknown model, openai-compat, which is NOT omit-safe so the field IS sent): learned/narrowed window 8,192 -> ceiling 5,053, reserve 2,730, but max_tokens sent = 8,192; total ask 13,245 on an 8,192 slot. Configured window 16,384 -> ceiling 10,104, reserve 5,461, max_tokens 8,192; total 18,296 on 16,384. Before #1179 this could not happen: the flat 20,000 reserve always exceeded any UNKNOWN_CAP or catalogued output ceiling. Now the reserve is under 8,192 for every window below 24,576 and under gpt-4o's 16,384 ceiling for every window below 49,152. GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. size_output_cap takes window_in_force and ends `sized.min(room(window))`, so the ask is bounded by the room the window in force actually leaves after the input the guard admitted. The clamp is IDENTITY wherever the window in force is the catalogued one (every registry model absent a #1172 narrowing), pinned by a_window_in_force_that_is_the_catalogued_one_changes_no_sizing -- the literal reading of this ticket's title, clamping the ask to the withheld reserve at every input, would cut a 200,000-window Claude turn from its real 64,000-token ceiling to the ~20,000-token compaction reserve. That choice is recorded as Q-1218 in .planning/DECISIONS.md. RED ARM (hetzner-dsm): replacing the final `match window_in_force` with a bare `sized` -- printed back after the edit, so it landed on the match expression and not on the 14 lines of comment above it -- reddens c2 and c3; restored, touched, green."
  - id: c2
    text: "A test asserts reserve >= the max_tokens that will be sent, across the window range 4,096..49,152 for an unlisted model; shown RED against today's UNKNOWN_CAP = 8,192"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::the_output_ask_never_outgrows_the_reserve_the_ceiling_withheld"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below. GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. Sweeps 4,096..49,152 in steps of 64 for an UNLISTED model (openai-compat/qwen3:8b), asserting at each window that the ask is no larger than output_reserve + emergency_buffer -- the reserve the ceiling withheld -- at the input the guard actually admits, and that est + ask <= window at five inputs spanning 0..ceiling. It carries its own control: it asserts model_output_ceiling(provider, model).is_none() first, so if qwen3:8b is ever catalogued the test says the case has moved rather than passing on the wrong arm."
  - id: c3
    text: "The measured cases here no longer hold: learned 8,192 -> ceiling 5,053 / reserve 2,730 / max_tokens 8,192, and configured 16,384 -> 10,104 / 5,461 / 8,192"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::the_measured_1218_overflows_no_longer_hold"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below. GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. The two measured cases, stated in the ticket's own figures: window 8,192 -> ceiling 5,053 / reserve 2,730, and window 16,384 -> ceiling 10,104 / reserve 5,461, both asserted as equalities so the arithmetic a re-grade reads is the arithmetic the ticket wrote. The ask is asserted != 8,192 (the measured value) and ceiling + ask <= window."
---

Scaling `output_reserve` down decoupled it from the `max_tokens` core actually asks for, so the pre-flight ceiling now certifies inputs that cannot fit alongside the output request. The scaled reserve is exactly w/3 (20,000 x 0.55w/33,000), while `size_output_cap` sizes `max_tokens` from the CATALOGUED window (or `UNKNOWN_CAP` = 8,192 for an unlisted model) and never sees the compact window or #1172's learned served window. Concretely, on the endpoint #1172 measured (unknown model, openai-compat, which is NOT omit-safe so the field IS sent): learned/narrowed window 8,192 -> ceiling 5,053, reserve 2,730, but max_tokens sent = 8,192; total ask 13,245 on an 8,192 slot. Configured window 16,384 -> ceiling 10,104, reserve 5,461, max_tokens 8,192; total 18,296 on 16,384. Before #1179 this could not happen: the flat 20,000 reserve always exceeded any UNKNOWN_CAP or catalogued output ceiling. Now the reserve is under 8,192 for every window below 24,576 and under gpt-4o's 16,384 ceiling for every window below 49,152.

**Where.** crates/wcore-agent/src/engine.rs:1148 `size_output_cap` (production call at crates/wcore-agent/src/engine.rs:13450) vs crates/wcore-config/src/compact.rs:541 `scaled_reserves` / :578 `input_ceiling_for_window`; the #255 guard at crates/wcore-agent/src/engine.rs:13237 compares only the input estimate

**Why it matters.** It undercuts #1179's own premise. The point of narrowing onto a learned window was to make a small served window safe, but the run then asks the endpoint for more output tokens than the whole window holds — on Ollama that generates until the context is exhausted, which re-creates the silent prompt truncation #1172 is about, this time with core reporting the request as inside its ceiling. Nothing asserts `scaled output_reserve >= the max_tokens that will actually be sent`.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
