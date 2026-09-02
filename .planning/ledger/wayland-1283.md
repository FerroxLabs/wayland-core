---
issue: 1283
repo: FerroxLabs/wayland
kind: defect
title: "Skills are still injected on every ordinary turn, with no relevance or activation gate (#1280 c3/c4/c5, #1150 c5 skills half)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Skills are injected only when relevant or explicitly activated (#1150 c5's text for the skills half), on a turn whose text relates to none of them, measured on the real bootstrap path"
    state: not-met
    evidence: test:crates/wcore-agent/tests/issue_1150_ordinary_turn_payload_test.rs::the_skills_listing_is_unconditional_on_an_ordinary_turn
    owner: core
    note: "FILED ffaa0d839 by lane/f13-s3-skills-ceiling as the decomposition of FerroxLabs/wayland#1280 c3, carried VERBATIM, and of the SKILLS half of FerroxLabs/wayland#1150 c5, which is superseded here. Nothing has been done on the code and this criterion is DELIBERATELY NOT SCHEDULED FOR 0.13.12 -- #1280's own Recommendation says c3/c4/c5 need a design decision before any code. WHAT IS ALREADY TRUE, so this is not re-derived: the listing is now BOUNDED (#1280 c1, shipped -- clamp_to_budget in wcore-skills/src/prompt.rs, graded by issue_1280_skills_ceiling_test.rs) and every trimmed skill stays reachable through Skill{query} (#1280 c2). It is still UNCONDITIONAL: context::format_skills_section emits the listing for every visible skill and nothing on that path consults relevance or activation. That is pinned, red-able, by the_skills_listing_is_unconditional_on_an_ordinary_turn in issue_1150_ordinary_turn_payload_test.rs, which asserts every planted skill is listed on a turn about arithmetic and which the #1150 lane proved can SEE a gate arriving (a crude half-the-skills gate reddens it 0 vs 10). SIZING, stated so this is not oversold: with the ceiling landed the listing costs at most 1% of the resolved window -- 1,310 chars on the reporter's 32,768-token session -- so a perfect gate saves at most that, where #1280 c1 removed a 15-17x overrun."
  - id: c2
    text: "WRONG-REFUSAL CONTROL for c1: on a turn that DOES need a gated-out skill, the model can still find and run it -- withholding a skill the model then cannot use is worse than a larger prompt"
    state: not-met
    evidence: test:crates/wcore-agent/tests/issue_1280_skills_ceiling_test.rs::a_trimmed_skill_is_found_and_run_on_a_turn_that_needs_it
    owner: core
    note: "FILED ffaa0d839. Carried verbatim from FerroxLabs/wayland#1280 c4. Nothing has been done; blocked behind c1's design decision by construction, since there is no gate yet to control. The shape to copy exists in-tree twice over and does not need designing: issue_1150_ordinary_turn_payload_test.rs (a_folded_out_tool_becomes_callable_on_explicit_activation) drives a real ToolSearch activation and asserts the folded-out tool is callable on the very next dispatch, and issue_1280_skills_ceiling_test.rs (a_trimmed_skill_is_found_and_run_on_a_turn_that_needs_it) does the same for a skill the ceiling trimmed -- discover via Skill{query}, invoke by exact name, assert the body comes back with is_error=false. A gate that withholds a skill the model then cannot use is worse than a larger prompt, and on this codebase's own standing rule the wrong refusal outranks the bytes it saves."
  - id: c3
    text: "Whatever c1 does, the prefix ahead of the conversation does not churn per turn on an implicit-cache endpoint, measured off the real LlmRequest across a multi-turn session"
    state: not-met
    evidence: file:.planning/ledger/wayland-1283.md
    owner: core
    note: "FILED ffaa0d839. Carried verbatim from FerroxLabs/wayland#1280 c5. Nothing has been done; blocked behind c1. THE VACUITY TRAP, carried because it has already caught one lane and would grade this criterion green while measuring nothing: crates/wcore-agent/tests/issue_1150_implicit_prefix_cache_test.rs is the obvious instrument and it is BLIND TO SKILLS. Its fixture sets Config::system_prompt directly (:119) and the engine takes that verbatim, so context::build_system_prompt -- the ONLY place the skills listing is assembled -- is never called on its path; segment 0 there is a fixed literal. MEASURED on lane/f13-w3-cache-spend: arm 1 replaced the cached skills section with an unconditional per-call listing carrying a turn counter, test PASSED; arm 2 additionally set cache.joined = None so the SystemPromptCache fast path could not swallow the churn, test PASSED again; arm 3, the POSITIVE CONTROL, appended a per-dispatch nonce to LlmRequest.system itself and the test FAILED instantly. So the oracle can see segment-0 churn and the two greens mean blind, not stable. It also aborts at the segment-0 equality assertion (:325) before it ever computes the breaks vector. This criterion needs an oracle whose fixture routes through build_system_prompt -- extend that file or write a new one, do not reuse it unchanged. DESIGN CONSTRAINTS, verified against source: build_system_prompt has exactly ONE call site (bootstrap.rs:2377) so the prompt is assembled at boot; moving assembly onto the dispatch path moves it out of segment 0 of an OpenAI-shaped body and, on #1150's own implicit-cache endpoint, re-bills the whole prompt uncached every turn -- which makes the reported symptom WORSE, and is what this criterion exists to catch; and #559 c6's recorded refusal forecloses moving skills into the message stream. Append-only, epoch-quantized activation -- the discipline admit_hydrated_tools already uses for tools -- remains the only shape that satisfies all three."
---

# The skills listing is bounded, and it is still unconditional

Decomposed out of `FerroxLabs/wayland#1280` on 2026-08-31, milestoned 0.13.13, because
that issue's own Recommendation separates a bounded fix from a feature and this is the
feature half. #1280 keeps c1/c2 -- the ceiling and its wrong-refusal control, both
shipped in 0.13.12. `FerroxLabs/wayland#1150` c5 is superseded here too: its TOOL half
is delivered and graded (that ledger's c7/c8), its SKILLS half is this issue's c1.

**What the ceiling did and did not do.** `format_skills_within_budget` no longer
subtracts the bundled block from the budget, no longer returns an all-bundled set
unconditionally, and no longer emits an unbounded names-only fallback: 100 bundled +
1,000 project skills went from 22,399 characters against a 1,310-character budget to
inside it, with a bounded overflow line naming what was withheld and `Skill { query }`
as the route back. That makes the listing BOUNDED. It leaves it UNCONDITIONAL --
assembled once at boot and shipped on every turn whatever the turn is about -- which is
the sentence #1150's reporter actually wrote and the sentence this issue owes.

**Why it is not a bounded fix.** The listing lives in the system prompt, which is
segment 0 of an OpenAI-shaped body and is built exactly once per session. A per-turn
relevance gate cannot live where it is assembled, and moving the assembly onto the
dispatch path moves it out of the cached prefix -- on the reporter's own implicit-cache
endpoint that re-bills every request in full and makes the reported symptom worse. c3
is the criterion that refuses that trade.
