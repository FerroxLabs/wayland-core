---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: 9ca5979e
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
    note: "MET, and it was graded by measurement rather than by reading. The engine syncs the canonical conversation once per stream ATTEMPT (engine.rs, `sync_journal_conversation(turn_id)` on the dispatch path), so by the time the output-stall gate stops the run both the prompt that started the turn and the tool round it produced are already durable; the test asserts both, on a real journal, through a full `run()`. A candidate fix -- adding `save_session_mirror()` at the stall exit to match the retry-exhausted exit ~120 lines below -- was written, red-armed, and DROPPED: removing it again left every arm green, and the configuration that would have needed it (a persisted session with no journal) is refused outright with 'persisted session has no exclusive journal writer lease'. It is recorded here because a line that survives its own red arm is a line that should not ship. What DID block this bullet was reachability, closed as c9: until that fix the stall gate could not run at all on a durable session, so nothing this criterion describes was ever exercised there. RECONCILED ON REBASE: integ/f13 reached this criterion first from lane/f13-fin-handoff-audit and graded it NOT-MET, on the finding that no issue anywhere was tracking a checkpoint-at-truncation and that session resume is a different object. That finding was correct about the TRACKING and is kept; what changed is that the property itself is now measured rather than searched for, by the test named above, on a real journal through a full run(). The earlier grade could not have been made by measurement, because until c9 the stall gate was unreachable on a durable session -- so there was no arm to measure. Graded met on the measurement, not on the reading. RED ARM: removing the per-attempt sync (both call sites) reddens this test and two others; on this one it reddens at the precondition -- `this arm must reach the output-stall gate: Err(SessionAuthority(\"invalid journal state transition: prepared request message count does not match the durable conversation\"))` -- so the mutation is proven to land on executable code but is not selective to the assertion, and this test is a regression guard rather than proof of a fix."
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
    note: "The limit exits admitted themselves and the FAILURE exits did not, and only the failure exits fire on this ticket's report. `emit_terminated_run_admission` is reached from `finish_run_terminated_inner`; a run that ends on a provider failure returns `Err` and never gets there, and its explanation goes out through `emit_error`, which is stderr. A `-p` run's stdout therefore ended on the model's last narration and read as a finished answer -- the Job-corpus A-10 failure the admission was written for, on the paths #388 actually reports. Fixed as a CLASS: the admission body is extracted to `emit_incomplete_run_admission` and called from the four terminal Err exits of the run loop (output stall, provider retry exhausted, unrepairable dispatch refusal, Flux loop-ownership collision) as well as from the limit exits, so the two wordings cannot drift. Deliberately NOT extended to `AgentError::UserAborted` (the user knows) or to the SessionAuthority exits (internal faults, several in helper contexts, no answer stream guaranteed live) -- 4 of 4 provider-failure exits changed, 0 of 4 abort exits, and the ~9 authority exits left alone and named here. RED ARM: making `emit_incomplete_run_admission` a no-op reddens 3 tests -- this one plus the two PRE-EXISTING limit-exit tests a_turn_capped_run_admits_it_on_the_answer_stream (engine_test.rs:444, `the answer stream must carry the admission, got: \"\"`) and a_guard_stop_is_not_reported_as_the_turn_cap (engine_test.rs:517) -- while the other 7 tests in that run stay green, which is the point: the extraction is shared, not duplicated. LIVE PROOF, added after this criterion was challenged: the unit test above grades the ENGINE, and the criterion is about where bytes land, so it was re-graded on the shipped binary rather than on the function. target/debug/wayland-core with a prompt on argv (one-shot mode, the -p equivalent) against a mock endpoint returning HTTP 400, stdout and stderr captured to separate files: stdout = 177 bytes containing ONLY \"[stopped early] I did not finish this: the provider refused this request and it could not be repaired into one worth re-sending. Anything above is partial work, not an answer.\", stderr = the two error lines and NO admission. Interleaved A/B on the same host, green-red-green, rebuilding between arms and touching after each edit: with emit_incomplete_run_admission stubbed to return early, the same command gives stdout = 0 BYTES, same exit code, same stderr. The mutation was proven to reach the artifact and not just the source -- `strings` on the relinked binary counts the admission string 1 in the green arm and 0 in the red, with a known-present control string staying at 1 in both. This also covers a SECOND of the four exits (the unrepairable dispatch refusal), which had no test; the stall exit is covered by the unit test above. All three production sinks forward the delta onto the answer stream -- TerminalSink (proven live here), ProtocolSink (Desktop json-stream) and ChannelSink (TUI) -- so the claim is not resting on the CLI alone."
  - id: c9
    text: "A stream retry after a FAILED tool round is possible on a durable session"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_failed_tool_round_still_retries_on_a_durable_session"
    owner: core
    note: "NEW, and it is the mechanism behind this ticket's headline symptom rather than one of its Expected-Behavior bullets. The clean-retry stub rewrites a FAILED tool-result body in the OUTBOUND copy of the request so a retry does not re-bill the full contaminated transcript. `commit_provider_recovery_checkpoint` then proves the prepared request against the durable conversation, and `validate_opened_prepared_request_conversation` refuses exactly that rewrite -- so on EVERY durable session (the default for Desktop and the CLI) a stream failure on a turn whose last tool round had failed could not retry. The run ended on `SessionAuthority(\"invalid journal state transition: prepared request message 2 changes durable content\")` in place of the provider's own error, and both the clean-retry stub and the output-stall progress gate were dead code there. Fixed by teaching the proof to accept that ONE rewrite, recomputed from the durable body and the tool name read out of the DURABLE conversation, so it admits nothing a prepared request can influence. Two controls in the test, neither decoration: control A flips one bit of the history (the tool round SUCCEEDED, so the stub does not fire) and proves the durable retry path is otherwise sound; control B runs the subject's exact scripts with no journal and proves the gate is alive off the durable path. RED ARM: an early `return false` in `is_retry_stub_of` reddens this test AND c6/c8's, with both controls still passing first -- subject message verbatim `Got: Err(Session persistence authority unavailable: invalid journal state transition: prepared request message 2 changes durable content)`."
  - id: c10
    text: "Expected-Behavior bullet 4 on the COMPACTION exits: a run that ends because its context could not be compacted says so where the ANSWER went"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_compaction_bail_admits_itself_on_the_answer_stream"
    owner: core
    note: "FOUND BY RE-GRADING c8 AGAINST ITS OWN SENTENCE, and it is the half c8 does not cover. c8 is worded \"a run that ends on a provider failure\", and that wording is honest about what it fixed -- but bullet 4 of the ticket says only \"clearly mark the task as failed/incomplete\", with no provider qualifier, and the reporter names this exact path in the body of the ticket: \"truncates because the model runs out of token budget\" IS the emergency ContextTooLong bail. Enumerated rather than assumed: run_inner_impl has 29 `return Err` sites; 4 are the provider-failure exits c8 fixed, 4 are UserAborted (deliberately excluded, the user knows), ~11 are SessionAuthority (internal faults, left alone), 6 are inside a nested provider closure and are not loop exits, and the remaining 3 are compaction failures -- all three the identical `prepare_durable_conversation / fire_on_session_end / cache_ledger.finish / save_session_mirror / return Err(e)` block. Those 3 said nothing on the answer stream. Fixed with the SAME shared helper and one shared wording, so a compaction exit and a provider exit cannot drift. RED ARM: deleting the three new call sites reddens a_compaction_bail_admits_itself_on_the_answer_stream at the assertion with the answer stream verbatim EMPTY (`: \"\"`), which is the measured symptom itself, while the control a_run_that_does_not_bail_carries_no_admission stays green (so the test cannot pass by admitting on every run) and c8 test stays green (so the mutation is scoped to this criterion). Restored, touched, re-run green."
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

c8 was challenged after this lane first graded it, and the challenge was worth
more than the grade: the criterion is about where BYTES land, and the evidence
was a unit test on the engine. Re-graded on the shipped binary instead. A
one-shot run against a mock endpoint that refuses writes 177 bytes to stdout,
all of it the admission, with the error on stderr and no admission there; stub
the admission out, rebuild, and the same command writes 0 bytes to stdout. That
is the defect in one line, and it is now measured rather than argued.

Re-grading it that way turned up c10. c8 says "provider failure", which is what
it fixed; bullet 4 says only "clearly mark the task as failed/incomplete". The
run loop has three more terminal Err exits — the compaction bails — and they
were silent on the answer stream. That is not a corner: "truncates because the
model runs out of token budget" is the reporter's own description of the
emergency bail. Closed with the same helper rather than filed, because it is
three call sites of a function that already existed and already had a live
proof behind it.

So the ticket now stays open on core (c7), not on flux.
