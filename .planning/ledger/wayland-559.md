---
issue: 559
repo: FerroxLabs/wayland
kind: defect
title: "Team leader token burn: 77.7M input tok/session, cache_read=0 — enable prompt caching + trim re-billed context (Core/Flux)"
status: open
last_verified_commit: 33167ed1fb64795d4fdbc8151a14153fb021098d
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
    state: blocked
    owner: desktop
    handoff: "FerroxLabs/wayland#1193"
    note: "the 0.0358 -> 0.6526 measurement is a 7-round-trip synthetic rig on flux-router. Closing on that proxy is exactly the substitution this ticket exists to catch. needs-platform-run: one real 26-turn Desktop team-leader session on a machine with the Desktop app, reading `cost_events` for that conversation_id and showing cache_read > 0. Cannot be produced from a Linux build host; c3/c5/c6 are closed underneath it and are what that run would now be measuring. PRECONDITION RE-CHECKED 2026-08-30 by lane w3-cache-spend, because this criterion Desktop run must not certify fabricated savings out of ledgers that are themselves mis-keyed. The three tickets that had to land first: wayland#1205 CLOSED and wayland#1206 CLOSED (verified via gh issue view, with wayland#1161 CLOSED as the known-positive control in the same query); wayland#1203 is FIXED on lane/f13-w3-cache-spend and is NOT yet on integ/f13. So the precondition does NOT hold at integ/f13 HEAD today, and it DOES hold the moment this lane merges. Stated that way deliberately: a Desktop run taken before the merge would produce exactly the spend-audit fragmentation #1203 describes -- a fresh launch and each --resume filed under different random uuids -- and any per-session saving totalled from that log would be an artefact. Nothing else about c4 changed; it still needs the real 26-turn Desktop team-leader run its own text asks for, which this lane cannot perform. --- 2026-08-31, relay-misc lane: RE-STATED AS blocked/desktop, verdict unchanged and no code moved. This row was not-met and owned by core, which reads as work core has not done yet. It is not: its own text is 'ONE real 26-turn Desktop team run', and a Desktop team-leader session cannot be produced from a Linux build host, from this repo, or by this lane -- the Desktop repo is read-only here and the run costs real spend on the maintainer's credential. THE CARRIER WAS SEARCHED FOR AND EXISTS: wayland#1193, 'Desktop: one real 26-turn team-leader run must show cache_read > 0 in cost_events (wayland#559 c4)', verified OPEN and carrying area:desktop + needs:desktop; known-positive control in the same pass, wayland#1161 -> CLOSED, so the query distinguishes state rather than returning a uniform answer. WHAT WOULD UNBLOCK IT, unchanged from the previous grading: one real 26-turn Desktop team-leader session, reading cost_events for that conversation_id and showing cache_read > 0 on turns after the first. The precondition tickets are wayland#1205 (CLOSED), wayland#1206 (CLOSED) and wayland#1203 -- which the 2026-08-30 note records as fixed on lane/f13-w3-cache-spend and NOT yet on integ/f13; it is still absent from this lane's base (ca15a48bf), so a Desktop run taken against this tree would still reproduce the spend-audit key fragmentation #1203 describes and any per-session saving totalled from it would be an artefact of the keying. So the run is owed AFTER that merge, not before."
  - id: c5
    text: "Ask 2's second half — the sub-call count is reduced, or shown not to need reducing"
    state: met
    evidence: "test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::every_sub_call_extends_the_previous_sub_calls_cached_prefix"
    owner: core
    note: "Shown not to need reducing, by measurement on the real dispatched requests rather than by argument. `test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::an_agentic_turn_dispatches_exactly_once_per_tool_round_plus_the_answer` pins the count at K tool rounds -> K+1 dispatches for K in 0,1,3,5: there is no hidden extra full-context sub-call to remove, so the count is already the minimum an agentic turn can do. What made turn 26 cost 4.88M input tokens was that each of those sub-calls re-billed the whole context UNCACHED, and the evidence test pins the property that fixes that instead: consecutive sub-calls are byte-identical up to the earlier one's cache write point, so sub-call N costs its delta. Not vacuous — the same file's `the_cached_prefix_still_extends_with_a_transient_tail` goes RED under the c6 mutation with `message 0 inside the cached prefix changed between sub-calls`, printing the plugin-context block present in one dispatch and gone in the next. RE-VERIFIED 2026-08-29: under the c6 mutation the_cached_prefix_still_extends_with_a_transient_tail panics at prompt_cache_prefix_test.rs:334 with `message 0 inside the cached prefix changed between sub-calls`, left carrying the <plugin-context source=\"test-plugin:contribute\"> PREPROMPT-CONTRIBUTION-9F3A block and right without it. an_agentic_turn_dispatches_exactly_once_per_tool_round_plus_the_answer and every_sub_call_extends_the_previous_sub_calls_cached_prefix both stay green under that mutation, which is the point: the count claim is independent of the boundary fix."
  - id: c6
    text: "The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::transient_carrier"
    owner: core
    note: "CLOSED POSITIONALLY, which is the only way this sentence can be discharged. RED ARM FIRST, on the production path: test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::the_transient_does_not_land_in_the_first_user_message_on_turn_one reads the LlmRequest the provider is actually handed, off the real run() path with a PrePrompt hook installed, and at the lane base c7f188c49 it FAILS with `turn 1 FIRST user message carries the transient contribution -- that is messages[1] on the wire, and it is re-sent without it on turn 2`, alongside moving_the_transient_off_the_first_message_does_not_drop_it (`left: 1, right: 2`). The other 8 of 10 in that file passed at base, so the file was not broken -- only these two claims were false. THE FIX: symbol:crates/wcore-agent/src/engine.rs::transient_carrier hands the skill-router hint and the PrePrompt contributions a dedicated trailing user message when the tail user message is ALSO the first one (messages.len() == 1, which is exactly turn 1), so the first user message is byte-stable from turn 1 onward and turn 1 finally writes an entry later turns can read. Both callers route through it; attach_transient_block itself is unchanged, so the placement rule INSIDE a message -- product wording never after the sender bytes in one flat blob -- is untouched. THE GATE IS compat.merge_same_role(), never a provider name (AGENTS.md rule 1). #1168 measured that a trailing transient message is merged straight back on the Anthropic family and recorded only two ways out, both needing a shared-converter change. There is a third, and it is this: do not emit a carrier onto a wire that cannot carry one. The presets setting merge_same_role are exactly anthropic, bedrock, vertex, minimax and gemini, and every one of them carries the system prompt in its OWN top-level field (system, systemInstruction) -- so on those wires messages[1] is not the first user turn on turn 1; it does not exist. On every wire where messages[1] IS the first user turn -- the OpenAI-shaped family, flux-router included, the family this ticket measured at cache_read = 0 -- the carrier applies. No converter changed, so nothing on the Anthropic path moved. c7 IS UNTOUCHED AND STILL MET, verified not asserted: its four tests run on common::test_config(), which is anthropic_defaults(), not one of their assertions was edited, and all four pass on this branch (10/10 in the file). The merging arm is graded too rather than assumed -- a_merging_wire_still_carries_the_transient_inside_the_first_message pins that no second adjacent user turn appears there. WRONG-REFUSAL CONTROL, in the same file: the cheapest way to pass a positional criterion is to stop injecting the content, which would be worse than the cache cost it removes, since the skill hint exists to steer the model FIRST action. moving_the_transient_off_the_first_message_does_not_drop_it asserts turn 1 still carries it, in a user-role carrier that is the tail. TWO NEIGHBOURS THIS TOUCHED, both graded. (1) recovery.rs admits the one extra trailing message under the same rule the append-onto-the-tail form already had -- user role, non-empty, text blocks only -- and with LESS latitude, since the carrier holds no durable content a prepared request could rewrite; the carrier timestamp is DERIVED from the message it follows, never Utc::now(), so a replay of the same turn digests identically. (2) untrusted_channel_wire_test read only the LAST user turn, which the carrier makes the product one; it now grades EVERY user turn -- the first is the sender and is still byte-exact, and every extra one must be product text the directive names and must carry NO sender byte. That re-anchor is not a widening, and two mutations on the production path prove it: an unnamed product string in the carrier fails with `openai: user turn 1 carries a line the system directive does not name`, and a sender nonce in it fails with `openai: a sender byte (PWN7Q2ZX-NONCE) reached the product own user turn`. unnamed_line_in is additionally graded directly over constructed inputs, the way the peel already is. engine.rs is a SOURCE_INPUT: the corpus was regenerated and key-diffed -- only source_inputs_digest and its derived fixture_digest moved, and schema_digest holds at sha256:8497e92e4ab2599201f95b2aa62c359ae2328429305e79a96761356483fc6e33."
  - id: c7
    text: "A turn-1 transient is never a cache WRITE point, so turn 1's entry stays readable by every later turn"
    state: met
    evidence: "test:crates/wcore-agent/tests/prompt_cache_prefix_test.rs::the_first_dispatch_writes_no_cache_entry_at_a_transient_head"
    owner: core
    note: "SPLIT OUT of c6 on 2026-08-29 -- this is the property that was actually delivered and measured, and it is graded here against its own text instead of against c6`s. #1168 could move the date into the system prefix because it is per-SESSION; the skill hint and PrePrompt contributions are per-TURN and derived from the turn, so there is no stable home for them. The two positional alternatives are both refused: a trailing transient message is what #1168 measured collisions on, and reordering them after the user's words breaks the invariant `attach_transient_block` exists to hold (trusted product context must not follow attacker-reachable text on a channel session). What is left, and what actually caused `cache_read = 0` on all 26 turns, is that the poisoned message was made a cache WRITE point: `apply_cache_zones` marked the tail unconditionally, so turn 1 wrote an entry no later turn could read. `symbol:crates/wcore-observability/src/cache.rs::mark_cache_boundaries` now stamps `MessageCacheHint::Transient` on such a tail and moves the write point back to the newest stable message; `symbol:crates/wcore-providers/src/anthropic.rs::request_has_transient_tail` carries the prohibition to the wire, where `apply_cache_zones` strips any marker left on it and shifts zones 3+4 back with it. Graded end to end on real dispatches with a live PrePrompt hook installed, plus `no_dispatch_ever_writes_a_cache_entry_at_a_transient_message` and the control `without_a_transient_contribution_the_tail_is_still_the_write_point`. RED ARM (mark_cache_boundaries ignoring the flag): 3 of the 7 go red, the control stays green. Residual stated out loud: on implicit-cache providers (OpenAI-shaped, incl. FluxRouter) there is no write point to move, so the transient still costs the tail message's prefix match each dispatch — bounded to one message and self-correcting, unlike the explicit-breakpoint collapse this closes. RE-VERIFIED 2026-08-29: the red arm (a `let transient_tail = false;` shadow at the top of mark_cache_boundaries) was re-applied and re-run. Exactly 3 of the 7 go red - the_first_dispatch_writes_no_cache_entry_at_a_transient_head (prompt_cache_prefix_test.rs:289, `left: [] right: [0]`), no_dispatch_ever_writes_a_cache_entry_at_a_transient_message (:305, `dispatch 0: the tail, and only the tail, is transient`, `left: [] right: [0]`), and the_cached_prefix_still_extends_with_a_transient_tail (:334) - while the control without_a_transient_contribution_the_tail_is_still_the_write_point stays green. Restored, touched, and re-run green 7/7."
---

Both root causes are fixed and the effect is measured: hit ratio 0.0358 ->
0.6526, monotonic, with no late collapse.

It stays open on c4 alone: that measurement is a synthetic 7-round-trip rig,
and this ticket measured a real 26-turn Desktop team leader. Its written
close condition is one real team run showing non-zero `cache_read`. That run
has not happened and cannot happen from a Linux build host.

c3 is no longer REFUTED. The refutation was a live measurement against ONE
provider read as a statement about all of them: Bedrock and Vertex were
handed cache breakpoints by the engine every turn and threw the system and
tools markers away, so caching was off at two of the three sites that support
it. The correction is recorded as an enumeration test rather than a third
measurement, because the failure mode here was a spot check standing in for a
list.

c6 and c7 were ONE criterion until 2026-08-29. It carried c6's positional
text — "no longer land at messages[1] on turn 1" — and was graded `met`
against the boundary property instead, which its own note stated openly. The
delivered work is real and is now c7: the poisoned message is no longer a
cache WRITE point, which is the part that made turn 1's write unreadable by
every later turn. The positional sentence is still false, so c6 is `not-met`.

That split is not bookkeeping. c6 arrives here from #1168 through a
`superseded` criterion, and both tickets rely on the same safeguard: the gate
refuses a handover whose successor is closed, and this ticket may not close
with c6 unmet. A c6 reading `met` against a substitute disarms both — and the
gate cannot catch a substituted property, only a missing anchor.

c6 is now met, and met against its own sentence rather than c7's. #1168 read
the choice as two shared-converter changes needing a maintainer — a
`merge_same_role` exemption for a transient tail, or an ordering change that
would break trusted-before-untrusted on channel sessions. Both are still
refused. The third option it did not enumerate needs no converter at all:
emit the carrier only onto a wire that can carry it, gated on
`compat.merge_same_role()`. The wires that cannot are exactly the ones whose
system prompt lives in its own top-level field, so `messages[1]` is not their
first user turn on turn 1 — and c7 already holds their write point off it.
