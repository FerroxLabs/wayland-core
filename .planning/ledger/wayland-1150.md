---
issue: 1150
repo: FerroxLabs/wayland
title: "[Bug]: Absurd Input Token Size"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "An unlisted model no longer gets a fabricated 200,000-token window; it sizes from the bottom of the range"
    state: met
    evidence: "symbol:crates/wcore-config/src/limits.rs::model_output_ceiling"
    owner: core
  - id: c2
    text: "Not every tool and MCP server is sent on every prompt"
    state: met
    evidence: "symbol:crates/wcore-tools/src/registry.rs::admit_hydrated_tools"
    owner: core
    note: "satisfied by MCP curation and tool deferral that shipped BEFORE this release, not by anything in it. Credit where due, but do not credit 0.13.10 for it"
  - id: c3
    text: "Large fetched content is truncated or summarised before it enters the context"
    state: met
    evidence: "test:crates/wcore-tools/src/web_fetch.rs::a_large_fetched_page_does_not_enter_the_context_whole"
    owner: core
    note: "WEB_FETCH_MAX_TEXT_CHARS = 20,000 caps text on a char boundary, flips truncated and adds truncation_notice; max_result_size was raised above the tool's own worst case so orchestration::truncate_result cannot mangle the JSON envelope. Negative control a_page_under_the_cap_is_untouched_and_not_marked_truncated is present."
  - id: c4
    text: "Accumulated prior tool RESULTS are not re-sent whole on every turn, and prompt/KV cache is reused where possible"
    state: met
    evidence: "test:crates/wcore-agent/src/compact/micro.rs::accumulated_tool_results_are_bounded_across_a_session"
    owner: core
    note: "Both halves. RESULTS: `symbol:crates/wcore-agent/src/compact/micro.rs::bound_accumulated_tool_results` is a ceiling on the SUM, wired into `run_compaction` step 0b. The gap it closes is precise — per-result truncation caps ONE result at ingestion (`Tool::max_result_size()`, 50,000 chars) and `microcompact` only clears old ones once real pressure reaches a fraction of the autocompact threshold, so between them N results at the cap ride at full size and are re-sent whole every turn. So this pass is ungated and applies to every tool, not just `compactable_tools`: a ceiling a tool can opt out of is not a ceiling. Guarantee: carried bytes never exceed `total_budget_bytes` plus the `keep_recent` newest results, both constants — the evidence test measures 20 AND 100 tool calls against the SAME ceiling, which is the claim (it stops growing with the session), and asserts a 20x shrink at 100. Monotone and epoch-quantized like the tool-call-args pass, so a bounded message never changes bytes again: `the_ceiling_is_byte_stable_on_a_second_pass`. Controls: `a_session_under_the_budget_is_untouched`, `the_ceiling_can_be_switched_off`. RED ARM: an early `return none` in the pass fails the evidence test on `the ceiling must have bitten`. CACHE REUSE: the second half is #559 c3/c5/c6, all three closed on the same branch — caching enabled on Bedrock and Vertex where it was off, per-turn transients kept out of every cache write point, and sub-call prefix stability measured on real dispatched requests."
---

Partially fixed in v0.13.10.

The compaction half landed: a model that is not in the limits table no longer
gets handed a fabricated 200,000-token window, which was silently
under-compacting and producing the input sizes this reporter saw.

c2 is recorded as met-but-not-by-this-release deliberately. Grading it as an
outstanding ask would be wrong; grading it as something 0.13.10 delivered
would be a false claim. Both are avoidable by saying which release did it.

c3's remainder — a quarter-megabyte web fetch entering the context untouched
— is closed, and c4 closes the shape behind it: not one oversized result, but
N capped ones summing without limit. The distinction matters because the two
fixes live in different places. c3 is a per-tool cap at ingestion; c4 is a
ceiling over history that runs before every dispatch, because no per-result
cap can bound a sum.
