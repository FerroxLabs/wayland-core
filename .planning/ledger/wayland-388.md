---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: 5639e5ff
criteria:
  - id: c1
    text: "Output caps are decided from what is actually known about the served model, not from the alias the request named"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "Reasoner replay is decided the same way, from what is known rather than from the alias"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c3
    text: "A prompt silently discarded by an under-sized served window is named to the user rather than showing as low pressure"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "shipped under #1172; a different cause of the same reported symptom"
  - id: c4
    text: "Router failure is distinguishable from an upstream provider rate limit, and a failed response names the arm that was actually serving it"
    state: blocked
    owner: flux
    handoff: "FerroxLabs/wayland#1184"
    note: "RE-SCOPED 2026-08-30. The old c4 read 'the remaining four bullets of this ticket's own Expected Behavior list are met' and handed the whole bag to flux. Two things are wrong with that. It is FOUR pieces of evidence under one id, which this ledger's own gate refuses ('a criterion needing two pieces of evidence is two criteria'), and the bag was misaddressed: #1184, which the core lane filed itself, quotes Flux saying the router-side truncate/stall causes are fixed and live and that 'the remaining asks are harness-side, not Flux'. The four bullets are split out below as c5, c6, c8 and c9 (all core) and c7 (core, not-met). What genuinely cannot be decided from this repo stays here: a router rate limit and an upstream provider rate limit arrive as the same non-2xx from the same host, and x-flux-routed-model is absent on failed responses, so anything core wrote would be a guess presented as a classification. The same re-scoping was reached independently on lane/f13-fin-handoff-audit; c4/c5/c6/c7 here are worded to converge with it rather than fork."
  - id: c5
    text: "Expected-Behavior bullets 2 and 5: a length-cut response stops before any write and commits no speculative file change"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_388_output_truncation_test.rs::complete_tool_calls_in_a_length_cut_response_are_not_executed"
    owner: core
    note: "Shipped and, until this audit, UNCREDITED -- it sat inside the old flux-owned c4, so two met bullets were being counted as somebody else's outstanding work. The #388(b) gate arms on the CUT rather than only on a call severed mid-argument, so a finish_reason=length response whose calls happened to close before the cut is treated as the prefix of a plan whose remainder was discarded: the calls are dropped and the turn is retried once with a smaller-steps hint. Two controls in the same file stop it passing by refusing everything -- a_length_cut_text_answer_without_tool_calls_still_commits and the_same_tool_call_runs_when_the_response_is_not_truncated -- and a third, a_call_cut_mid_argument_still_aborts, holds the older half."
  - id: c6
    text: "Expected-Behavior bullet 3: a truncated or stalled long task preserves a checkpoint the user can continue from"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_stalled_run_leaves_its_conversation_durable"
    owner: core
    note: "MET, and it was graded by measurement rather than by reading. The engine syncs the canonical conversation once per stream ATTEMPT (engine.rs, `sync_journal_conversation(turn_id)` on the dispatch path), so by the time the output-stall gate stops the run both the prompt that started the turn and the tool round it produced are already durable; the test asserts both, on a real journal, through a full `run()`. A candidate fix -- adding `save_session_mirror()` at the stall exit to match the retry-exhausted exit ~120 lines below -- was written, red-armed, and DROPPED: removing it again left every arm green, and the configuration that would have needed it (a persisted session with no journal) is refused outright with 'persisted session has no exclusive journal writer lease'. It is recorded here because a line that survives its own red arm is a line that should not ship. What DID block this bullet was reachability, closed as c9: until that fix the stall gate could not run at all on a durable session, so nothing this criterion describes was ever exercised there. RED ARM: removing the per-attempt sync (both call sites) reddens this test and two others; on this one it reddens at the precondition -- `this arm must reach the output-stall gate: Err(SessionAuthority(\"invalid journal state transition: prepared request message count does not match the durable conversation\"))` -- so the mutation is proven to land on executable code but is not selective to the assertion, and this test is a regression guard rather than proof of a fix."
  - id: c7
    text: "Expected-Behavior bullet 7: the failure categories core CAN decide are machine-readable rather than a free-form string"
    state: not-met
    owner: core
    handoff: "FerroxLabs/wayland#1237"
    note: "MISLABELLED as flux until this audit -- also inside the old c4. crates/wcore-protocol/src/events.rs:2289-2293 is `ErrorInfo { code: String, message: String, retryable: bool }`: no typed category, no provider identity, no upstream status. Three of the five categories this ticket names -- context/token limit, tool/runtime failure, local Wayland error -- are decidable INSIDE core today and still reach the host as prose, so only the router-versus-provider split actually needs #1184. DECOMPOSED to #1237 rather than closed here: it is a contract addition to a pinned protocol surface, it overlaps wayland-core#314 c5 (the same shape on the grant-refusal path), and it is not the same work as the harness fixes this lane shipped. #1237 carries the criteria."
  - id: c8
    text: "Expected-Behavior bullet 4: a run that ends on a provider failure says so where the ANSWER went, not only on stderr"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::an_output_stall_stop_is_reachable_and_admits_itself"
    owner: core
    note: "The limit exits admitted themselves and the FAILURE exits did not, and only the failure exits fire on this ticket's report. `emit_terminated_run_admission` is reached from `finish_run_terminated_inner`; a run that ends on a provider failure returns `Err` and never gets there, and its explanation goes out through `emit_error`, which is stderr. A `-p` run's stdout therefore ended on the model's last narration and read as a finished answer -- the Job-corpus A-10 failure the admission was written for, on the paths #388 actually reports. Fixed as a CLASS: the admission body is extracted to `emit_incomplete_run_admission` and called from the four terminal Err exits of the run loop (output stall, provider retry exhausted, unrepairable dispatch refusal, Flux loop-ownership collision) as well as from the limit exits, so the two wordings cannot drift. Deliberately NOT extended to `AgentError::UserAborted` (the user knows) or to the SessionAuthority exits (internal faults, several in helper contexts, no answer stream guaranteed live) -- 4 of 4 provider-failure exits changed, 0 of 4 abort exits, and the ~9 authority exits left alone and named here. RED ARM: making `emit_incomplete_run_admission` a no-op reddens 3 tests -- this one plus the two PRE-EXISTING limit-exit tests a_turn_capped_run_admits_it_on_the_answer_stream (engine_test.rs:444, `the answer stream must carry the admission, got: \"\"`) and a_guard_stop_is_not_reported_as_the_turn_cap (engine_test.rs:517) -- while the other 7 tests in that run stay green, which is the point: the extraction is shared, not duplicated."
  - id: c9
    text: "A stream retry after a FAILED tool round is possible on a durable session"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_failed_tool_round_still_retries_on_a_durable_session"
    owner: core
    note: "NEW, and it is the mechanism behind this ticket's headline symptom rather than one of its Expected-Behavior bullets. The clean-retry stub rewrites a FAILED tool-result body in the OUTBOUND copy of the request so a retry does not re-bill the full contaminated transcript. `commit_provider_recovery_checkpoint` then proves the prepared request against the durable conversation, and `validate_opened_prepared_request_conversation` refuses exactly that rewrite -- so on EVERY durable session (the default for Desktop and the CLI) a stream failure on a turn whose last tool round had failed could not retry. The run ended on `SessionAuthority(\"invalid journal state transition: prepared request message 2 changes durable content\")` in place of the provider's own error, and both the clean-retry stub and the output-stall progress gate were dead code there. Fixed by teaching the proof to accept that ONE rewrite, recomputed from the durable body and the tool name read out of the DURABLE conversation, so it admits nothing a prepared request can influence. Two controls in the test, neither decoration: control A flips one bit of the history (the tool round SUCCEEDED, so the stub does not fire) and proves the durable retry path is otherwise sound; control B runs the subject's exact scripts with no journal and proves the gate is alive off the durable path. RED ARM: an early `return false` in `is_retry_stub_of` reddens this test AND c6/c8's, with both controls still passing first -- subject message verbatim `Got: Err(Session persistence authority unavailable: invalid journal state transition: prepared request message 2 changes durable content)`."
---

Graded against this ticket's own Expected Behavior list: 3 of 7 bullets are
met at v0.13.10, which is why it stays open.

Core's half was that output caps and reasoner replay were being decided from
`request.model` — the alias the caller typed — rather than from the model the
router actually served. `0cab1cf8` decides both from what is known.

#1172 closed a third, independent cause of the same user-visible complaint (an
endpoint silently discarding the prompt), which is worth reading alongside this
before anyone re-grades it.

**Re-graded 2026-08-30, and the old c4 was wrong in both directions.** It said
"the remaining four bullets are met" and gave all four to flux. Two of those
bullets were already SHIPPED by core and going uncredited (c5), one is met and
was gradeable all along (c6), one needed a small class fix that this lane made
(c8), and only the failure-ORIGIN half is genuinely unobservable from this repo
(c4, carried by #1184). The typed-category half is core's and is decomposed to
#1237 (c7).

The finding that actually matters is c9, and it is not on the ticket's bullet
list at all: on every durable session — the default for Desktop and the CLI —
a stream failure on a turn whose last tool round had failed could not retry.
The clean-retry stub rewrites the failed tool-result body in the outbound
request; the provider-dispatch proof refuses exactly that rewrite; the run died
on an internal journal error in place of the provider's own. Both the clean
retry and the output-stall progress gate — the engine's answer to "it stalls
and burns tokens without progress", which is the title of this ticket — were
unreachable code there. Any grading of the stall behaviour done before that fix
was grading a path that could not execute.

So the ticket now stays open on core (c7), not on flux.
