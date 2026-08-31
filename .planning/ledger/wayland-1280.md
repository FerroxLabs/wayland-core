---
issue: 1280
repo: FerroxLabs/wayland
kind: defect
title: "The skills listing is injected on every ordinary turn, and its 1%-of-window budget is not a ceiling (#1150 c5, skills half)"
status: open
last_verified_commit: 33e3edde1
criteria:
  - id: c1
    text: "The skills listing respects a ceiling derived from the resolved context window, with no term that grows without bound in the skill count: bundled entries are inside the budget rather than subtracted from it, and the names-only fallback is itself bounded. Graded at 100 bundled / 1,000 project skills"
    state: not-met
    evidence: "symbol:crates/wcore-skills/src/prompt.rs::format_skills_within_budget"
    owner: core
    note: "Filed 2026-08-31 as the skills half of FerroxLabs/wayland#1150 c5, split out by lane f13-ctx-1150 rather than half-built in the #1150 lane -- that refusal was correct. LEDGERED 2026-08-31 during the f13 landing pass, which is when the coverage gate caught that this issue was OPEN and in scope with no ledger file at all. Nothing has been done on the code. THE MEASUREMENT, from the issue body and not re-derived here: format_skills_within_budget computes remaining_budget = budget.saturating_sub(bundled_chars), which SUBTRACTS the bundled entries and never caps them, and the minimal fallback still emits every bundled skill at its FULL description plus every non-bundled NAME. Both terms grow linearly in the skill count and neither is bounded by the window. Against the 1,310-char budget a 32,768-token window implies: 300 project skills -> 5,999 chars (4.6x); 1,000 project -> 19,999 (15.3x); 100 bundled -> 22,399 (17.1x, about 5,600 tokens). This is a live context-burn defect of exactly the class 0.13.12 exists to close, and the issue's own Recommendation says c1/c2 is a bounded fix with a real measured win and no design question in it. Kept at 0.13.12 for that reason."
  - id: c2
    text: "WRONG-REFUSAL CONTROL for c1: a skill trimmed out of the listing is still reachable -- the model can discover and invoke it, measured on a session where it actually needs one, or the trimming is refused"
    state: not-met
    evidence: "symbol:crates/wcore-skills/src/prompt.rs::format_skills_within_budget"
    owner: core
    note: "Filed 2026-08-31. Nothing has been done. This is the control that decides whether c1's fix is a win or a regression, and it is not optional: a listing bounded by silently dropping skills the model then cannot reach is a WRONG REFUSAL, which on this codebase's own standing rule outranks the leak it closes. The tool half of #1150 c5 already carries the equivalent control -- issue_1150_ordinary_turn_payload_test.rs drives a real ToolSearch activation and asserts the folded-out tool becomes callable on the very next dispatch -- so the shape to copy exists in-tree and does not need designing."
  - id: c3
    text: "Skills are injected only when relevant or explicitly activated (#1150 c5's text for the skills half), on a turn whose text relates to none of them, measured on the real bootstrap path"
    state: not-met
    evidence: "file:.planning/ledger/wayland-1280.md"
    owner: core
    note: "Filed 2026-08-31. Nothing has been done, and this criterion is DELIBERATELY NOT SCHEDULED FOR 0.13.12. The issue's own Recommendation is explicit that c3/c4/c5 need a design decision before any code and that on the measured numbers they are worth far less than c1: a relevance gate saves at most the size of a BOUNDED listing, while c1 is what stops that listing being 17x its stated budget in the first place. Recorded here rather than dropped so the split is visible: c1/c2 are the 0.13.12 work, c3/c4/c5 are a feature owing a design decision and should be decomposed to their own issue under 0.13.13. Until that split is filed they stay on this ledger and are honestly not-met -- a criterion nobody split is a partial ticket, which this file refuses to pretend otherwise about."
  - id: c4
    text: "WRONG-REFUSAL CONTROL for c3: on a turn that DOES need a gated-out skill, the model can still find and run it -- withholding a skill the model then cannot use is worse than a larger prompt"
    state: not-met
    evidence: "file:.planning/ledger/wayland-1280.md"
    owner: core
    note: "Filed 2026-08-31. Nothing has been done. Blocked behind c3's design decision by construction: there is no gate yet to control. See c3 for the 0.13.13 split rationale."
  - id: c5
    text: "Whatever c3 does, the prefix ahead of the conversation does not churn per turn on an implicit-cache endpoint, measured off the real LlmRequest across a multi-turn session"
    state: not-met
    evidence: "file:.planning/ledger/wayland-1280.md"
    owner: core
    note: "Filed 2026-08-31. Nothing has been done. Blocked behind c3. ONE THING THE NEXT LANE MUST NOT REUSE BLINDLY, carried from the issue body because it is a live vacuity trap: issue_1150_implicit_prefix_cache_test.rs sets Config::system_prompt directly, so build_system_prompt is never called on its path and its segment-0 assertion is BLIND TO SKILLS. It needs extending, not reusing as-is; reused as-is it would grade this criterion green while measuring nothing about skills at all."
---

# The skills listing is unbounded, and it ships on every turn

Split out of `FerroxLabs/wayland#1150` c5 by the lane that refused to half-build it. #1150's
tool half is delivered: `defer_cold` folds 40 of 48 tool schemas out of `tools[]` into a
566-byte catalog line, 52,110 schema bytes down to 8,902, with a wrong-refusal control that
drives a real activation and asserts the folded-out tool is callable on the next dispatch.
The skills half has no gate at all.

Two separate defects live in this ticket, and the issue's own Recommendation separates them:

**The budget is not a ceiling (c1/c2).** `format_skills_within_budget` computes
`remaining_budget = budget.saturating_sub(bundled_chars)` -- it SUBTRACTS the bundled entries
instead of capping them -- and the names-only fallback still emits every bundled skill at full
description plus every non-bundled name. Both terms grow linearly in the skill count. Against
the 1,310-char budget a 32,768-token window implies: 1,000 project skills render 19,999 chars
(15.3x); 100 bundled render 22,399 (17.1x, about 5,600 tokens of a 32,768-token window). That
is a bounded fix with a measured win and no design question in it, which is why it is on
0.13.12.

**No relevance gate (c3/c4/c5).** `context.rs:254 format_skills_section` emits the listing
unconditionally; nothing anywhere consults relevance or activation. Closing this needs a design
decision first, and on the measured numbers it is worth far less than c1 -- a gate saves at
most the size of a BOUNDED listing, while c1 is what stops that listing being 17x its stated
budget. These three should be decomposed onto their own issue under 0.13.13; until that split
is filed they stay here and stay honestly not-met, because a criterion nobody split is a
partial ticket.

Both halves carry a wrong-refusal control on purpose. A listing bounded by silently dropping
skills the model then cannot reach is a worse defect than the prompt bytes it saves.
