---
issue: 1150
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Absurd Input Token Size"
status: open
last_verified_commit: 31597b00
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
    note: "Both halves. RESULTS: `symbol:crates/wcore-agent/src/compact/micro.rs::bound_accumulated_tool_results` is a ceiling on the SUM, wired into `run_compaction` step 0b. The gap it closes is precise — per-result truncation caps ONE result at ingestion (`Tool::max_result_size()`, 50,000 chars) and `microcompact` only clears old ones once real pressure reaches a fraction of the autocompact threshold, so between them N results at the cap ride at full size and are re-sent whole every turn. So this pass is ungated and applies to every tool, not just `compactable_tools`: a ceiling a tool can opt out of is not a ceiling. Guarantee: carried bytes never exceed `total_budget_bytes` plus the `keep_recent` newest results, both constants — the evidence test measures 20 AND 100 tool calls against the SAME ceiling, which is the claim (it stops growing with the session), and asserts a 20x shrink at 100. Monotone and epoch-quantized like the tool-call-args pass, so a bounded message never changes bytes again: `the_ceiling_is_byte_stable_on_a_second_pass`. Controls: `a_session_under_the_budget_is_untouched`, `the_ceiling_can_be_switched_off`. RED ARM: an early `return none` in the pass fails the evidence test on `the ceiling must have bitten`. CACHE REUSE: the second half is #559 c3/c5/c6, all three closed on the same branch — caching enabled on Bedrock and Vertex where it was off, per-turn transients kept out of every cache write point, and sub-call prefix stability measured on real dispatched requests. RE-VERIFIED 2026-08-29: the red arm was re-applied (an unconditional `return none;` immediately after the enabled checks in bound_accumulated_tool_results) and re-run - accumulated_tool_results_are_bounded_across_a_session panics at micro.rs:2076 with `the ceiling must have bitten`. Restored, touched, and re-run green: all 6 ceiling tests pass, including the two controls and the epoch/byte-stability pair. RE-GRADED 2026-08-30: the RESULTS half stands as written and its red arm was re-confirmed by the sweep. The CACHE half is no longer graded here - it was borrowed from an open ticket and is now c5, with its own in-tree evidence on the reporter's provider shape."
  - id: c5
    text: "prompt/KV cache is reused where possible"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1150_implicit_prefix_cache_test.rs::every_dispatch_extends_the_previous_dispatchs_byte_prefix"
    owner: core
    note: "SPLIT OUT OF c4 on 2026-08-30, because c4 is two claims and one evidence token cannot anchor both - the ledger's own rule that a criterion needing two pieces of evidence is two criteria. c4's evidence covers the tool-RESULTS half; this is the CACHE half, and it was previously closed by delegation to wayland#559 c3/c5/c6. That delegation had three problems, all confirmed at e7144c30a: #559 is still open and its own c4 is not-met; #559 c6 closed an admitted substitution; and #559 c6's recorded residual says 'on implicit-cache providers there is no write point to move' - which is #1150's reporter's exact shape (LM Studio over an OpenAI-compatible endpoint), where no measurement existed at all. VERIFIED at HEAD: wcore-observability::cache::mark_cache_boundaries returns before writing any breakpoint when compat.cache_message_breakpoints() is false, so #559's instrument reads ZERO on this route - an_openai_shaped_route_places_no_cache_write_point asserts that as the precondition. Where there is no write point, reuse is a property of the BYTES, so the claim is measured on the real LlmRequest the engine hands the provider: across a 4-turn, 16-dispatch session every dispatch repeats the previous dispatch's messages verbatim and appends (#559's own file stops at the sub-calls inside ONE turn; the reported session was 26). a_bounded_tool_result_is_rewritten_once_and_then_frozen measures the interaction with c4's first half over 18 dispatches - a tool result goes verbatim->stub ONCE and is then frozen, and the boundary is epoch-quantized. RED ARMS, all on executable code in compact/micro.rs, diff read back before each run, file touched after mutate and after restore: (A) `if total <= tr.total_budget_bytes` and `if running <= tr.total_budget_bytes` -> `if false` reddens every_dispatch_extends_the_previous_dispatchs_byte_prefix with 'dispatch 6 diverges from dispatch 5 at message 2 of 11'; (B) dropping the `!*stubbed` monotone filter reddens a_bounded_tool_result_is_rewritten_once_and_then_frozen with 'message 2 changed bytes 3 times across the session'; (C) `let epoch = tr.epoch_results.max(1)` -> `let epoch = 1` reddens the quantization bound with '8 of 17 dispatch pairs'. Arm C is recorded because the FIRST cut of that bound survived it - a gate that could not fail - and was re-pinned from the measurement (quantized 5, unquantized 8). All three green after restore."
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
