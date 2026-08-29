---
issue: 559
repo: FerroxLabs/wayland
kind: defect
title: "Team leader token burn: 77.7M input tok/session, cache_read=0 — enable prompt caching + trim re-billed context (Core/Flux)"
status: open
last_verified_commit: 7381d875
criteria:
  - id: c1
    text: "The turn-1 transient that poisoned the cache prefix is removed from messages[1]"
    state: met
    evidence: "commit:6762f218"
    owner: core
    note: "tracked in full as #1168"
  - id: c2
    text: "The OpenAI adapter no longer drops accompanying text on tool-result turns"
    state: met
    evidence: "commit:d6f64be2"
    owner: core
    note: "d6f64be2 is the red arm; the second root cause found in stage-2"
  - id: c3
    text: "Ask 1 — enable prompt caching where it was off"
    state: met
    evidence: "test:crates/wcore-config/src/config.rs::every_breakpoint_provider_defaults_prompt_caching_on"
    owner: core
    note: "The earlier REFUTED reading was one site generalized to all of them. `prompt_caching` resolved to `matches!(provider, Anthropic)`, but Bedrock and Vertex both set `cache_message_breakpoints: Some(true)` in ProviderCompat — the engine computed cache boundaries for them every turn and the adapters dropped the system + tools markers on the floor (`bedrock.rs` / `vertex.rs`, `if self.cache_enabled`). Caching was OFF at two of the three sites that support it. Fixed by `symbol:crates/wcore-config/src/config.rs::prompt_caching_on_by_default`, which covers the whole Anthropic family; the evidence test is an ENUMERATION over every ProviderType, kept complete by `all_is_complete_and_correctly_ordered`, so a future breakpoint-honouring provider cannot be added without it. RED ARM: reverting the default to Anthropic-only fails it — `Bedrock honours cache breakpoints but defaults prompt_caching OFF`. The off path was fixed too: both adapters leaked the engine hint through `build_messages` with caching disabled (`test:crates/wcore-providers/src/bedrock.rs::caching_off_leaks_no_marker_from_the_engine_hint`). Sites deliberately left alone: OpenAI-shaped endpoints cache implicitly, Gemini does not honour breakpoints, MiniMax is off with a documented unverified-beta-header reason and says so in its compat. RE-VERIFIED 2026-08-29 on lane/f13-prompt-cache: the red arm was re-run, not inherited. Reverting prompt_caching_on_by_default to matches!(provider, ProviderType::Anthropic) fails every_breakpoint_provider_defaults_prompt_caching_on at config.rs:11922 with `Bedrock honours cache breakpoints but defaults prompt_caching OFF - the engine would mark boundaries the adapter then drops`, and takes the_anthropic_family_defaults_on_and_the_rest_do_not with it (2 of 5 red); all_is_complete_and_correctly_ordered stays green, so the enumeration itself is not what broke. Restored and re-run green (49/49 in wcore-config + wcore-observability)."
  - id: c4
    text: "This ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read"
    state: superseded
    owner: desktop
    handoff: FerroxLabs/wayland#1193
    note: "DECOMPOSED 2026-08-29 to FerroxLabs/wayland#1193 (open, area:desktop + needs:desktop), which carries the full contract. Not blocked -- core's half of this criterion is empty, and that was ESTABLISHED rather than assumed. Three measurements on integ/f13: (1) cost_events does not exist in wayland-core -- a grep across every .rs, .md, .sql and .ts in the repo returns exactly one hit and it is this ledger file quoting the criterion; the table is the Desktop app's own SQLite, at ~/Library/Application Support/Wayland/wayland/wayland.db, which is the path #559's own repro line names. (2) There is no team leader in wayland-core: team.leader / team_leader returns three hits, all of them prose comments REFERENCING this issue, and there is no team subcommand on the CLI. (3) The build host is Linux and the Desktop app is not on it. So the CLI could produce a proxy and not the artifact -- and this criterion's whole purpose is to refuse a proxy, so core did not produce one. The core-side cache ledger was checked for a pre-existing real run rather than taken on trust: of 1,732,292 files under the hetzner cache-ledger, the 13,975 over 3 KB were grepped for a non-zero cache_read_tokens and every hit is model 'test-model' at 5 turns -- synthetic. No real session on a Linux host has ever recorded a cache read, which is consistent with the fixes being correct and the host having no Desktop team to drive them. THE CONTRACT ON #1193: one team-leader conversation of at least 26 turns on a Desktop build carrying c1/c2/c3/c5/c6 (0.13.12 or later, or integ/f13 at ab6b602f or newer), then SELECT turn, input_tokens, cache_read FROM cost_events WHERE conversation_id = '<id>' ORDER BY turn, pasted, with cache_read > 0 on turns after the first. It must also state the MODEL: the original ran backend wcore / model flux-reasoning, and FluxRouter is OpenAI-shaped and caches implicitly, so core maps prompt_tokens_details.cached_tokens onto cache_read (crates/wcore-providers/src/openai.rs:2110) and the column will populate -- but c6 moves an EXPLICIT write point, and an implicit-cache provider has none to move, so on FluxRouter the lift is c1/c2/c3 while an Anthropic-family model exercises the breakpoint path as well. #1193 also names what a red means: cache_read still 0 on a build carrying the fixes is a NEW defect handed back to core with the conversation_id, the turn table and the model, not a reopen of c3/c5/c6, because each of those is pinned by a test with a re-run red arm."
  - id: c5
    text: "Ask 2's second half — the sub-call count is reduced, or shown not to need reducing"
    state: met
    evidence: "test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::every_sub_call_extends_the_previous_sub_calls_cached_prefix"
    owner: core
    note: "Shown not to need reducing, by measurement on the real dispatched requests rather than by argument. `test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::an_agentic_turn_dispatches_exactly_once_per_tool_round_plus_the_answer` pins the count at K tool rounds -> K+1 dispatches for K in 0,1,3,5: there is no hidden extra full-context sub-call to remove, so the count is already the minimum an agentic turn can do. What made turn 26 cost 4.88M input tokens was that each of those sub-calls re-billed the whole context UNCACHED, and the evidence test pins the property that fixes that instead: consecutive sub-calls are byte-identical up to the earlier one's cache write point, so sub-call N costs its delta. Not vacuous — the same file's `the_cached_prefix_still_extends_with_a_transient_tail` goes RED under the c6 mutation with `message 0 inside the cached prefix changed between sub-calls`, printing the plugin-context block present in one dispatch and gone in the next. RE-VERIFIED 2026-08-29: under the c6 mutation the_cached_prefix_still_extends_with_a_transient_tail panics at prompt_cache_prefix_test.rs:334 with `message 0 inside the cached prefix changed between sub-calls`, left carrying the <plugin-context source=\"test-plugin:contribute\"> PREPROMPT-CONTRIBUTION-9F3A block and right without it. an_agentic_turn_dispatches_exactly_once_per_tool_round_plus_the_answer and every_sub_call_extends_the_previous_sub_calls_cached_prefix both stay green under that mutation, which is the point: the count claim is independent of the boundary fix."
  - id: c6
    text: "The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1"
    state: not-met
    owner: core
    note: "RE-GRADED 2026-08-29 from met to not-met. The criterion is POSITIONAL -- no longer land at messages[1] on turn 1 -- and this branch does not satisfy it. The branch's OWN tests say so. the_transient_contribution_is_present_then_gone_on_the_next_sub_call (crates/wcore-agent/tests/prompt_cache_prefix_test.rs:255-275) asserts for EVERY dispatch, dispatch 0 included, that the tail carries the PrePrompt contribution; and the_first_dispatch_writes_no_cache_entry_at_a_transient_head (:279-295) asserts that on turn 1 messages.len() == 1 and transient_indices(first) == vec![0] -- the transient is still in the turn-1 head. (The criterion's messages[1] counts the system prompt as messages[0]; LlmRequest carries system as its own field, so the same slot is request.messages[0]. Either way it is the front of the prefix.) This ledger's own prose already said it: the transient still sits inside the turn-1 user message. What was closed is a DIFFERENT, SUBSTITUTED property -- that the message is never a cache WRITE point -- and grading the substitute as the criterion is how a partial ships. THE SUBSTITUTED PROPERTY IS REAL AND STAYS PROVEN; nobody should redo it. mark_cache_boundaries (crates/wcore-observability/src/cache.rs:51-85) stamps MessageCacheHint::Transient on a transient tail and moves the write point back with checked_sub(2), returning None when that leaves nothing; the stamp is applied even when compat.cache_message_breakpoints() is false because it is a PROHIBITION, not a request (:47-50, :61-66); anthropic.rs::request_has_transient_tail carries it to the wire, where apply_cache_zones strips any leftover marker and shifts zones 3 and 4 back. The red arm (a let transient_tail = false; shadow at the top of mark_cache_boundaries) turns exactly 3 of the 7 red -- :289, :305, :334 -- while the control without_a_transient_contribution_the_tail_is_still_the_write_point stays green. That work is what c5 leans on, and c5 is met. WHY THIS IS NOT PEDANTRY -- two consequences. (a) HANDOVER: wayland#1168 c3 is superseded INTO this criterion, on the explicit ground that the residual is not closed by this change and is carried as a live criterion on #559 rather than left in prose, because a residual nobody can find is a residual nobody fixes. Closing it here on a restated contract is exactly the move that handover exists to prevent, and it would be the residual's second silent hop. (b) IT IS AN UNFIXED, CUSTOMER-FACING TOKEN BURN ON IMPLICIT-CACHE PROVIDERS. Verified on this tree: ProviderType::FluxRouter resolves to ProviderCompat::flux_router_defaults(), which builds on openai_compat_provider and never sets cache_message_breakpoints, so cache_message_breakpoints() is false (compat.rs:1234-1236); and crates/wcore-providers/src/openai.rs contains ZERO references to MessageCacheHint. On those endpoints there is no write point to move, so the fix above is a no-op and the defect is untouched. The transient is injected into the per-turn CLONE of history, so on turn 2 the turn-1 head is byte-different from what turn 1 actually sent -- a change at message index 0, the front of the prefix an implicit-cache endpoint matches on -- and turn 2 therefore reuses none of the messages array. From turn 3 on the change has moved deeper (the previous turn's user message), so the loss is bounded to one message and self-correcting, but the turn-2 break is total. FluxRouter is the provider this ticket's own 0.0358 -> 0.6526 rig ran on and the one the reported team-leader session used, so the family this residual falls on is the family the ticket was filed about. REMAINING WORK, core-owned, and why it is NOT decomposed into a second ticket: any change that satisfies the criterion as written -- keep per-turn transient content out of the turn-1 head -- fixes explicit- and implicit-cache providers together, so there is no half that can go invisible behind the other, and #559 is not closing anyway (c4 is its own unmet close condition). Splitting here would hand the residual a third hop and buy no visibility that not-met plus owner core does not already buy from the release gate. The property to build toward: every message index a dispatch has ALREADY SENT stays byte-identical in the next dispatch. The two shapes previously refused stay refused for the stated reasons (a trailing transient MESSAGE is what #1168 measured collisions on; reordering the transient after the user's words breaks the trusted-before-untrusted invariant attach_transient_block exists to hold). Not yet evaluated, and offered as the honest next step rather than as a decision: retaining the transient in persisted history instead of injecting it into a per-turn clone -- nothing is then rewritten retroactively and the prefix is stable by construction, at a context-growth cost that has to be measured. Grade the fix with a two-turn byte-diff of the dispatched messages arrays on an OpenAI-shaped compat, not with a write-point assertion."
---

Both named root causes are addressed, and the effect is measured on the
provider family that honours cache breakpoints: hit ratio 0.0358 -> 0.6526,
monotonic, with no late collapse.

c4 is the ticket's own close condition and it is DECOMPOSED, not blocked. The
7-round-trip rig is a synthetic proxy, and this ticket measured a real 26-turn
Desktop team leader; closing on the proxy is the substitution it exists to
catch. But core's half of c4 is empty rather than pending: `cost_events` is the
Desktop app's own SQLite table, there is no team leader anywhere in
wayland-core, and the build host is Linux. So the remainder went to
FerroxLabs/wayland#1193 on the desktop lane (open, `area:desktop` +
`needs:desktop`) with the run, the query, the expected table and the model
caveat written down, and with what a red means and where it comes back to.

c6 is NOT part of that handover and stays open on core. It was graded `met`
against a substituted property — a cache-boundary fact standing in for the
positional one the criterion states — and has been re-graded `not-met`; see
below. The desktop run under #1193 will EXERCISE the c3, c5 and c6 fixes as a
side effect of being a real session, but it cannot close c6: c6 is a claim
about where the transient sits in `messages`, which is decidable here, and
`prompt_cache_prefix_test.rs` still asserts `transient_indices(first) ==
vec![0]` on turn 1.

c3 is no longer REFUTED. The refutation was a live measurement against ONE
provider read as a statement about all of them: Bedrock and Vertex were
handed cache breakpoints by the engine every turn and threw the system and
tools markers away, so caching was off at two of the three sites that support
it. The correction is recorded as an enumeration test rather than a third
measurement, because the failure mode here was a spot check standing in for a
list.

c6 was closed as a boundary fact rather than a positional one, and that was
wrong: the criterion is positional, and the transient still sits inside the
turn-1 user message. It has been re-graded `not-met`. The boundary work is
real, fully proven, and stays recorded in c6's note and under c5 — but it is a
different property, and grading it as c6 restated the contract to fit the fix.

Two things make that more than bookkeeping. #1168 handed this over through a
`superseded` criterion precisely so the residual would be carried live until
someone closed it, and the gate refuses that handover if this ticket ever
closes with c6 unmet. And the residual lands on implicit-cache providers —
FluxRouter among them, the endpoint this ticket's own measurement ran on —
where there is no write point to move, so the turn-1 head still changes under
turn 2 and the whole messages prefix misses. That is an unfixed, customer-facing
token burn, not a rounding error.

c6 stays one criterion rather than being split: closing it as written fixes
both provider families at once, so neither half can hide behind the other.
