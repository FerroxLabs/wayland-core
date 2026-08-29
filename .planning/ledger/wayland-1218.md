---
issue: 1218
repo: FerroxLabs/wayland
kind: defect
title: "The scaled output_reserve is decoupled from the max_tokens core actually sends, so the pre-flight ceiling admits a request that cannot fit"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "size_output_cap and scaled_reserves agree: the max_tokens sent is never larger than the reserve the ceiling withheld"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37, found while verifying FerroxLabs/wayland#1179). Nothing has been done. The measured finding, verbatim: Scaling `output_reserve` down decoupled it from the `max_tokens` core actually asks for, so the pre-flight ceiling now certifies inputs that cannot fit alongside the output request. The scaled reserve is exactly w/3 (20,000 x 0.55w/33,000), while `size_output_cap` sizes `max_tokens` from the CATALOGUED window (or `UNKNOWN_CAP` = 8,192 for an unlisted model) and never sees the compact window or #1172's learned served window. Concretely, on the endpoint #1172 measured (unknown model, openai-compat, which is NOT omit-safe so the field IS sent): learned/narrowed window 8,192 -> ceiling 5,053, reserve 2,730, but max_tokens sent = 8,192; total ask 13,245 on an 8,192 slot. Configured window 16,384 -> ceiling 10,104, reserve 5,461, max_tokens 8,192; total 18,296 on 16,384. Before #1179 this could not happen: the flat 20,000 reserve always exceeded any UNKNOWN_CAP or catalogued output ceiling. Now the reserve is under 8,192 for every window below 24,576 and under gpt-4o's 16,384 ceiling for every window below 49,152."
  - id: c2
    text: "A test asserts reserve >= the max_tokens that will be sent, across the window range 4,096..49,152 for an unlisted model; shown RED against today's UNKNOWN_CAP = 8,192"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The measured cases here no longer hold: learned 8,192 -> ceiling 5,053 / reserve 2,730 / max_tokens 8,192, and configured 16,384 -> 10,104 / 5,461 / 8,192"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D37). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

Scaling `output_reserve` down decoupled it from the `max_tokens` core actually asks for, so the pre-flight ceiling now certifies inputs that cannot fit alongside the output request. The scaled reserve is exactly w/3 (20,000 x 0.55w/33,000), while `size_output_cap` sizes `max_tokens` from the CATALOGUED window (or `UNKNOWN_CAP` = 8,192 for an unlisted model) and never sees the compact window or #1172's learned served window. Concretely, on the endpoint #1172 measured (unknown model, openai-compat, which is NOT omit-safe so the field IS sent): learned/narrowed window 8,192 -> ceiling 5,053, reserve 2,730, but max_tokens sent = 8,192; total ask 13,245 on an 8,192 slot. Configured window 16,384 -> ceiling 10,104, reserve 5,461, max_tokens 8,192; total 18,296 on 16,384. Before #1179 this could not happen: the flat 20,000 reserve always exceeded any UNKNOWN_CAP or catalogued output ceiling. Now the reserve is under 8,192 for every window below 24,576 and under gpt-4o's 16,384 ceiling for every window below 49,152.

**Where.** crates/wcore-agent/src/engine.rs:1148 `size_output_cap` (production call at crates/wcore-agent/src/engine.rs:13450) vs crates/wcore-config/src/compact.rs:541 `scaled_reserves` / :578 `input_ceiling_for_window`; the #255 guard at crates/wcore-agent/src/engine.rs:13237 compares only the input estimate

**Why it matters.** It undercuts #1179's own premise. The point of narrowing onto a learned window was to make a small served window safe, but the run then asks the endpoint for more output tokens than the whole window holds — on Ollama that generates until the context is exhausted, which re-creates the silent prompt truncation #1172 is about, this time with core reporting the request as inside its ceiling. Nothing asserts `scaled output_reserve >= the max_tokens that will actually be sent`.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
