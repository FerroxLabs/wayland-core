---
issue: 1150
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Absurd Input Token Size"
status: open
last_verified_commit: 9de21aa1
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
    state: not-met
    evidence: "test:crates/wcore-agent/src/compact/micro.rs::accumulated_tool_results_are_bounded_across_a_session"
    owner: core
    note: "Both halves. RESULTS: `symbol:crates/wcore-agent/src/compact/micro.rs::bound_accumulated_tool_results` is a ceiling on the SUM, wired into `run_compaction` step 0b. The gap it closes is precise — per-result truncation caps ONE result at ingestion (`Tool::max_result_size()`, 50,000 chars) and `microcompact` only clears old ones once real pressure reaches a fraction of the autocompact threshold, so between them N results at the cap ride at full size and are re-sent whole every turn. So this pass is ungated and applies to every tool, not just `compactable_tools`: a ceiling a tool can opt out of is not a ceiling. Guarantee: carried bytes never exceed `total_budget_bytes` plus the `keep_recent` newest results, both constants — the evidence test measures 20 AND 100 tool calls against the SAME ceiling, which is the claim (it stops growing with the session), and asserts a 20x shrink at 100. Monotone and epoch-quantized like the tool-call-args pass, so a bounded message never changes bytes again: `the_ceiling_is_byte_stable_on_a_second_pass`. Controls: `a_session_under_the_budget_is_untouched`, `the_ceiling_can_be_switched_off`. RED ARM: an early `return none` in the pass fails the evidence test on `the ceiling must have bitten`. CACHE REUSE: the second half is #559 c3/c5/c6, all three closed on the same branch — caching enabled on Bedrock and Vertex where it was off, per-turn transients kept out of every cache write point, and sub-call prefix stability measured on real dispatched requests. RE-VERIFIED 2026-08-29: the red arm was re-applied (an unconditional `return none;` immediately after the enabled checks in bound_accumulated_tool_results) and re-run - accumulated_tool_results_are_bounded_across_a_session panics at micro.rs:2076 with `the ceiling must have bitten`. Restored, touched, and re-run green: all 6 ceiling tests pass, including the two controls and the epoch/byte-stability pair. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: FIRST HALF HOLDS. `bound_accumulated_tool_results` (micro.rs:647) is wired ungated at run_compaction step 0b (engine.rs:18098) and compact_now (engine.rs:6791); run_compaction itself runs at the top of every turn iteration (engine.rs:12842, comment 'Run multi-level compaction before each API call'). RAN: 6 ceiling tests green — accumulated_tool_results_are_bounded_across_a_session, the_newest_results_are_never_dropped_by_the_ceiling, a_session_under_the_budget_is_untouched, the_ceiling_can_be_switched_off, the_ceiling_is_byte_stable_on_a_second_pass, the_ceiling_advances_in_epoch_sized_batches, plus the_ceiling_applies_to_tools_outside_the_compactable_list. The ledger's red-arm claim is credible: micro.rs:2074 is the `assert!(result.cleared_count > 0, 'the ceiling must have bitten')` the note quotes (the note says :2076, off by two lines — cosmetic drift, message matches). SECOND HALF DOES NOT HOLD as an independently verified claim. The criterion text is 'and prompt/KV cache is reused where possible'. The stated evidence (a micro.rs tool-result test) says nothing about caching; the ledger delegates it entirely to wayland#559 c3/c5/c6. Three problems with that delegation: (1) #559 is still OPEN in the same ledger directory and its own written close condition — c4, 'ONE real 26-turn Desktop team run showing non-zero cache_read' — is recorded `not-met` / needs-platform-run, so #1150 c4 is leaning on a ticket that has not itself been proven end to end; (2) #559 c6 is closed by the lane's own words as 'a cache-boundary fact, not a positional one' against a criterion whose text is 'no longer land at messages[1] on turn 1' — an admitted substitution, and the exact pattern this sweep was told to catch; (3) most materially for THIS reporter, #559 c6's own recorded residual states that 'on implicit-cache providers (OpenAI-shaped, incl. FluxRouter) there is no write point to move'. #1150's reporter is on LM Studio over an OpenAI-compatible endpoint — precisely the provider shape where the delivered cache work does the least, and no measurement exists on any OpenAI-shaped endpoint. I confirmed both #559 evidence symbols resolve (config.rs:1085 prompt_caching_on_by_default, config.rs:12068 every_breakpoint_provider_defaults_prompt_caching_on; prompt_cache_prefix_test.rs:219 and :280), so the delegated work is real — it is the CLOSURE that is borrowed from an open ticket, not the code."
  - id: c5
    text: "Tool schemas AND SKILLS are injected only when relevant or explicitly activated, rather than on every ordinary chat turn"
    state: not-met
    owner: core
    note: "ADDED 2026-08-29 by the 0.13.12 close-sweep. This half of the issue had NO criterion, so it was invisible in the 'all criteria met' reading. Recorded verbatim: TICKET-VS-LEDGER DRIFT. The issue's Expected Behavior says, verbatim: 'Tool schemas and skills should be injected only when relevant or explicitly activated, rather than on every ordinary chat turn.' The reporter's payload inventory also names 'active skills' as its own line item alongside the 29 MCP tool definitions. No ledger criterion covers skills. c2 was narrowed to 'Not every tool and MCP server is sent on every prompt' — the tool half only. In the tree, skills are still listed unconditionally on every turn: context.rs:192 `format_skills_section` emits a </system-reminder> listing for every visible skill, and wcore-skills/src/prompt.rs:61 `format_skills_within_budget` only DEGRADES the listing (full -> truncated descriptions -> names-only) against a char budget. There is no relevance gate and no explicit-activation gate anywhere on that path. That half of the ticket's ask is neither delivered nor graded, so it is invisible in the 'all criteria met' reading. It is not a false claim by the lane — it is an unrepresented ask, which is why it only surfaces when you read the ticket instead of the ledger."
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
