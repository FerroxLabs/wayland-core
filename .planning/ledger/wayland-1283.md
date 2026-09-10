---
issue: 1283
repo: FerroxLabs/wayland
kind: defect
title: "Skills are still injected on every ordinary turn, with no relevance or activation gate (#1280 c3/c4/c5, #1150 c5 skills half)"
status: open
last_verified_commit: 6488f9762
criteria:
  - id: c1
    text: "Skills are injected only when relevant or explicitly activated (#1150 c5's text for the skills half), on a turn whose text relates to none of them, measured on the real bootstrap path"
    state: met
    evidence: test:crates/wcore-agent/tests/issue_1283_skill_activation_gate_test.rs::an_ordinary_turn_names_no_installed_skill
    owner: core
    note: "MET 6488f9762 (lane w15/skills). THE REPAIR, in crates/wcore-agent/src/context.rs::build_system_prompt: the per-skill listing is gone from the boot prompt and what stands in its place is SKILL_DISCOVERY_SECTION, a compile-time constant that names no skill, counts none and consults no turn. The gate is only `does this session have at least one model-invocable skill` -- a session with none emits nothing, a session with any emits the same fixed sentence -- so the only skill content that ever reaches the model is what the model asked for. WHY NOT A PER-TURN RELEVANCE GATE, the alternative this criterion is often read as asking for: build_system_prompt has exactly ONE call site, at boot (bootstrap.rs:2427), and the String it returns is segment 0 of every request for the life of the session. A gate that varies per turn cannot live where the listing is assembled, and moving assembly onto the dispatch path moves it out of the cached prefix and re-bills the whole prompt uncached on the reporters own implicit-cache endpoint -- which is what c3 refuses and is a LARGER cost than the listing. GRADED ON THE REAL BOOTSTRAP PATH, with no Config::system_prompt shortcut: an_ordinary_turn_names_no_installed_skill boots AgentBootstrap::build() with 10 planted skills and a recording provider, asks `What is 2 + 2?`, and asserts none of the 10 is named in LlmRequest.system on that turn or on a later one. NON-VACUITY, three independent facts because `no skill name in the prompt` is also what a session with no skills looks like: the discovery block is present (it is emitted only when a model-invocable skill was discovered, so its presence proves the catalogue loaded); the_skills_the_gate_withholds_are_genuinely_installed drives a live Skill{query} on the same fixture and asserts the planted names come back; and the prompt is asserted non-trivial. FAIL-BEFORE: the oracle was committed FIRST, at fdfd387c8, referencing no symbol the repair adds so it compiles against the defect -- 3 of its 5 tests RED there (c1, c2 and the cache arm), 26/26 green at 0ad56529b after. RED ARM ON THE FIX: MUTANT M2 (or_insert_with(String::new), i.e. no discovery instructions at all) reds 6 tests including this one on its non-vacuity assertion; MUTANT M1 (a per-render nonce) reds only the cache arm. SIZING, stated so this is not oversold: the constant is about 470 characters against a listing budget of 1,310 on a 32,768-token window, so this is a saving on a machine with many skills installed and a SMALL COST on one with two or three. The point of the change is the conditionality, not the bytes -- #1280 c1 already removed the 15-17x overrun. WHAT THIS DOES NOT DO: there is no active-skill epoch. The guides `if active metadata must enter the trusted prompt` is answered NO -- the Skill tool already returns the body as a tool result, so nothing about an activated skill needs to enter segment 0, and the epoch count is therefore zero. If a future change needs one, it must be append-only and rebuilt only on actual activation."
  - id: c2
    text: "WRONG-REFUSAL CONTROL for c1: on a turn that DOES need a gated-out skill, the model can still find and run it -- withholding a skill the model then cannot use is worse than a larger prompt"
    state: met
    evidence: test:crates/wcore-agent/tests/issue_1283_skill_activation_gate_test.rs::a_withheld_skill_is_found_and_run_on_a_turn_that_needs_it
    owner: core
    note: "MET 6488f9762 (lane w15/skills). This is the control that decides whether c1 is a win or a wrong refusal, and it is measured on a session that genuinely needs a withheld skill: a_withheld_skill_is_found_and_run_on_a_turn_that_needs_it in crates/wcore-agent/tests/issue_1283_skill_activation_gate_test.rs, 1,000 planted skills, real engine dispatches. IT GRADES TWO THINGS, and the second is the one a scripted provider cannot supply on its own. (1) THE ROUTE IS STATED: the prompt names the Skill tool, its `query` parameter, its `skill` parameter, and ToolSearch -- the hop that is needed because Skill is not on defer_colds hot allowlist and is folded out of tools[] on a default session. (2) THE ROUTE IS EXECUTABLE: Skill is asserted ABSENT from dispatch 0s tools[], ToolSearchs own catalog is asserted to name it, dispatch 1 is asserted to carry it, Skill{query: reticulate splines} is asserted to name m-skill-777 out of a thousand, and Skill{skill: m-skill-777} is asserted to return its body with is_error=false -- all read off ContentBlock::ToolResult in the NEXT LlmRequest, not off a helper. The escape hatch stays bounded (<= SKILL_SEARCH_MAX_RESULTS hits), so it does not undo #1280 c1s ceiling on the message-stream side. WHY (1) IS NOT DECORATION: a scripted provider runs the query whether or not the prompt ever told a real model the route exists, so an oracle that only drove the chain would stay GREEN with the discovery instructions deleted -- and a prompt that withholds every name AND says nothing about how to search is exactly the wrong refusal this criterion forbids. That is the required disable-Skill-discovery mutant and it was run: MUTANT M2 at e78897a95 replaced the section with String::new, `cargo fmt` clean, the mutation verified on CODE (context.rs:514, inside the or_insert_with closure), and this test RED with its own message -- verbatim, `the prompt withholds every installed skill and says nothing about how to find one. That is the wrong refusal this criterion forbids, whatever the tool layer can still do.` #1280 c2s sibling control a_trimmed_skill_is_found_and_run_on_a_turn_that_needs_it reddened with it, 6 red of 15. Reset to d5f4c5bae, tree clean, green. PERMISSIONS PRESERVED, checked rather than assumed: the search runs over SkillCatalog::visible() and invocation goes through resolve_for_model, so disable_model_invocation skills are unreachable through this route exactly as before, and a hidden-only catalogue emits no discovery block at all (context.rs test_build_system_prompt_all_hidden_no_reminder). LATE-LOADED SKILLS PRESERVED: format_skills_section is UNCHANGED and still renders for late_mcp.rs:132 and the engines transient inventory-change block, so MCP skills arriving after boot are still merged into the shared catalog, still announced, and still searchable. THE LIMIT: this proves the route is stated and executable. It does not prove a real model READS the instruction and searches often enough -- no synthetic oracle can, and that is a live-usage question."
  - id: c3
    text: "Whatever c1 does, the prefix ahead of the conversation does not churn per turn on an implicit-cache endpoint, measured off the real LlmRequest across a multi-turn session"
    state: met
    evidence: test:crates/wcore-agent/tests/issue_1283_skill_activation_gate_test.rs::the_skills_section_is_identical_across_two_different_catalogues
    owner: core
    note: "MET 6488f9762 (lane w15/skills), in TWO arms whose split is stated because a lane already shipped two greens off an oracle blind to what it was pointed at. ARM A, the_system_prefix_is_identical_across_turns_and_an_activation: a 4-dispatch session over 3 user turns, one of which ToolSearches for Skill and then invokes m-skill-777, asserts LlmRequest.system is byte-identical across every dispatch; the activation is asserted to have HAPPENED (a dispatch carries the skill body) so it is not an ordinary-turns arm wearing an activations name, and the message stream is asserted to GROW so the equality is a fact about the prefix and not about a frozen fixture. ARM A IS A REGRESSION GUARD AND SAYS SO IN ITS OWN DOC: within-session stability is structural today (one call site, at boot), so it would go red only if someone moved assembly onto the dispatch path -- exactly the trade this criterion refuses. ARM B, the_skills_section_is_identical_across_two_different_catalogues, is the arm with teeth: an implicit cache is keyed on the prefix and shared across sessions, so two boots over deliberately different catalogues (10 skills named a-skill-*, 1,000 named m-skill-*) must produce a byte-identical prefix. It compares the WHOLE prefix with only the per-session tempdir normalized out. THE REQUIRED NONCE MUTANT, AND WHAT IT CAUGHT: MUTANT M1 appends a per-render nonce to the skills section. On the FIRST cut of arm B -- which compared the extracted <system-reminder> span -- the nonce landed one line outside the span and the arm stayed GREEN on a tree whose prefix demonstrably churned. That is the same vacuity that let issue_1150_implicit_prefix_cache_test.rs pass twice while blind. The arm was rewritten to compare the whole prefix (4d01e681a, d5f4c5bae) and M1 re-run at 2dd60a073: RED, and the panic quotes the mutant text -- boot A ends the section with MUTANT-NONCE-0 and boot B with MUTANT-NONCE-1 -- so the mutation is proven to have landed on executed CODE and not on a comment. 14 of 15 other tests stayed green, so M1 is isolated to this arm. Reset to d5f4c5bae, tree clean, green. WHY THE PREFIX CANNOT CHURN: the skills section is a compile-time constant, so it is identical across turns, across activations, across catalogues and across machines. THE LIMIT, stated: no LlmRequest is sent to a real implicit-cache endpoint here, so what is measured is the byte-identity of the prefix the engine hands a provider, not a cache-hit rate. A SEPARATE FACT FOUND BY ARM B AND NOT FIXED HERE: the prefix embeds a per-project memory path, so two projects can never share a cached prefix whatever the skills do."
---

# The skills listing was bounded; it is no longer unconditional

Decomposed out of `FerroxLabs/wayland#1280` on 2026-08-31, milestoned 0.13.13, because
that issue's own Recommendation separates a bounded fix from a feature and this is the
feature half. #1280 keeps c1/c2 -- the ceiling and its wrong-refusal control, both
shipped in 0.13.12. `FerroxLabs/wayland#1150` c5 is superseded here too: its TOOL half
is delivered and graded (that ledger's c7/c8), its SKILLS half is this issue's c1.

**What the ceiling did and did not do.** `format_skills_within_budget` no longer
subtracts the bundled block from the budget, no longer returns an all-bundled set
unconditionally, and no longer emits an unbounded names-only fallback. That made the
listing BOUNDED. It left it UNCONDITIONAL -- assembled once at boot and shipped on every
turn whatever the turn was about -- which is the sentence #1150's reporter actually wrote
and the sentence this issue owed.

**What closed it.** The listing is out of the trusted prefix entirely and what stands in
its place is `context::SKILL_DISCOVERY_SECTION`: a compile-time constant stating that
skills are installed, that `Skill { query }` searches every one of them, that
`Skill { skill }` runs one by exact name, and that `ToolSearch { query: "Skill" }` loads
the tool when it is cold. Nothing is withheld -- the registry is searchable and invocable
exactly as before, over `SkillCatalog::visible()`, so hidden and revoked skills stay
hidden. `format_skills_section` is UNCHANGED and still renders for its two other
production call sites, `late_mcp.rs` and the engine's transient inventory-change block,
so late-arriving MCP skills are still merged, announced and searchable.

**Why not a per-turn relevance gate.** `build_system_prompt` has exactly one call site,
at boot. A gate that varies per turn cannot live where the listing is assembled, and
moving assembly onto the dispatch path moves it out of the cached prefix -- on the
reporter's own implicit-cache endpoint that re-bills every request in full and makes the
reported symptom worse. c3 is the criterion that refuses that trade, and it is now graded
by an oracle that routes through the real bootstrap rather than the one that was blind to
skills by construction.

## What this change did to OTHER issues' evidence, stated rather than left to be found

Three ledgers this lane does not own grade properties that were measured on the boot
prompt's skills listing. The listing is gone, so those measurements had to move. None of
them was weakened silently and none of their evidence tokens was renamed:

- **`FerroxLabs/wayland#1199` c2/c3** (closed, `met`) cite
  `an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes` and
  `the_bootstrap_prompt_uses_the_real_window_derived_skill_budget`. Both read the
  window-derived listing off `engine.system_prompt()`. Through that door the assertion
  would now pass on ANY budget, including the 8,000-character fabrication #1199 exists to
  keep out. Both now render through `format_skills_section` using the session's own
  `known_context_window()` and the real boot-discovered catalogue -- which is the chain
  those criteria claim ("the real window-derived budget reaches the renderer", not
  "reaches segment 0"). The hop no longer covered is boot-prompt assembly, because there
  is no listing there to cover. **wayland-1199.md's notes should be amended by whoever
  owns it.**
- **`FerroxLabs/wayland#1150` c5** (superseded) cites
  `the_skills_listing_is_unconditional_on_an_ordinary_turn`. That function now asserts the
  INVERSE of what its name says. The name is kept because the ledger gate resolves every
  `test:` token to a declared `fn` and that file is not this lane's to edit; the body
  carries a loud comment saying so and naming the rename to make
  (`an_ordinary_turn_carries_no_skills_listing`).
- **`FerroxLabs/wayland#1280` c1** was graded partly on the bootstrap path
  (`a_thousand_project_skills_still_fit_the_window_budget`). That arm now asserts the boot
  prompt renders NO listing, and measures the ceiling on `format_skills_section` instead,
  which is where the surviving production call sites are. #1280's own evidence tokens are
  symbols in `wcore-skills/src/prompt.rs` and are untouched.

**A pre-existing vacuity found on the way.** #1199's precondition "the planted skill
reached the listing in BOTH arms" was read off the system prompt and matched
`"issue-1150"` -- which is also the first two segments of `UNLISTED_MODEL`, stated in
every prompt's intro. An 80-character budget cannot hold one entry, so the tight arm never
contained the planted skill and the precondition passed on the model name. It is now
asymmetric and true: the roomy arm HOLDS the skill, the tight arm DECLARES that it
dropped it.
