---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: e4100643a
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
    note: "The limit exits admitted themselves and the FAILURE exits did not, and only the failure exits fire on this ticket's report. `emit_terminated_run_admission` is reached from `finish_run_terminated_inner`; a run that ends on a provider failure returns `Err` and never gets there, and its explanation goes out through `emit_error`, which is stderr. A `-p` run's stdout therefore ended on the model's last narration and read as a finished answer -- the Job-corpus A-10 failure the admission was written for, on the paths #388 actually reports. Fixed as a CLASS: the admission body is extracted to `emit_incomplete_run_admission` and called from the four terminal Err exits of the run loop (output stall, provider retry exhausted, unrepairable dispatch refusal, Flux loop-ownership collision) as well as from the limit exits, so the two wordings cannot drift. Deliberately NOT extended to `AgentError::UserAborted` (the user knows) or to the SessionAuthority exits (internal faults, several in helper contexts, no answer stream guaranteed live) -- 4 of 4 provider-failure exits changed, 0 of 5 abort exits, and the 12 authority exits left alone and named here (the earlier '~9' in this note was a third number for the same set inside one ledger; the recount under c10 is the one that holds). THAT EXCLUSION WAS WRONG and is closed by c12: 'no answer stream guaranteed live' was a reading, and when it was measured -- a mid-stream `LlmEvent::Error` carrying the journal-authority prefix, through a full production `run()` -- the answer stream held the narration and nothing else. The 12 authority exits were the reported defect, not an exception to it. RED ARM: making `emit_incomplete_run_admission` a no-op reddens 3 tests -- this one plus the two PRE-EXISTING limit-exit tests a_turn_capped_run_admits_it_on_the_answer_stream (engine_test.rs:444, `the answer stream must carry the admission, got: \"\"`) and a_guard_stop_is_not_reported_as_the_turn_cap (engine_test.rs:517) -- while the other 7 tests in that run stay green, which is the point: the extraction is shared, not duplicated. LIVE PROOF, added after this criterion was challenged: the unit test above grades the ENGINE, and the criterion is about where bytes land, so it was re-graded on the shipped binary rather than on the function. target/debug/wayland-core with a prompt on argv (one-shot mode, the -p equivalent) against a mock endpoint returning HTTP 400, stdout and stderr captured to separate files: stdout = 177 bytes containing ONLY \"[stopped early] I did not finish this: the provider refused this request and it could not be repaired into one worth re-sending. Anything above is partial work, not an answer.\", stderr = the two error lines and NO admission. Interleaved A/B on the same host, green-red-green, rebuilding between arms and touching after each edit: with emit_incomplete_run_admission stubbed to return early, the same command gives stdout = 0 BYTES, same exit code, same stderr. The mutation was proven to reach the artifact and not just the source -- `strings` on the relinked binary counts the admission string 1 in the green arm and 0 in the red, with a known-present control string staying at 1 in both. This also covers a SECOND of the four exits (the unrepairable dispatch refusal), which had no test; the stall exit is covered by the unit test above. The earlier form of this note said 'all three production sinks' and under-enumerated: the non-test OutputSink impls that forward a text delta are TerminalSink (output/terminal.rs), ProtocolSink (output/protocol_sink.rs), ChannelSink (agents/channel_sink.rs), the ACP engine (wcore-cli/src/acp_engine.rs) and the TUI bridge (wcore-cli/src/tui/engine_bridge.rs), plus NullSink (output/null_sink.rs) which no-ops by contract. Five forward it, not three. Only TerminalSink is proven LIVE here; the other four are read, not measured, and that gap is residual #4 rather than part of this grade."
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
    note: "FOUND BY RE-GRADING c8 AGAINST ITS OWN SENTENCE, and it is the half c8 does not cover. c8 is worded \"a run that ends on a provider failure\", and that wording is honest about what it fixed -- but bullet 4 of the ticket says only \"clearly mark the task as failed/incomplete\", with no provider qualifier, and the reporter names this exact path in the body of the ticket: \"truncates because the model runs out of token budget\" IS the emergency ContextTooLong bail. Enumerated rather than assumed -- and CORRECTED after a verifier recounted it, because the enumeration this criterion rests on was WRONG and the arithmetic that 'summed exactly' summed to the wrong total. Recounted at c511d32b7 over run_inner_impl (then engine.rs 12350-17121; the next fn at that indent is try_live_workflow, and there are no nested fn definitions in the range): 32 `return Err` sites, not 29. RE-DERIVED INDEPENDENTLY at e4100643a, where the same function spans 12403-17180 and still yields 32 -- but see c12, which retires this census as the thing the class claim rests on: `return Err(` was never the whole exit set, because 39 more lines in the same span carry a `?` try-operator and each of those is a terminal Err exit too. 12 are SessionAuthority (internal faults, left alone), 5 are UserAborted (deliberately excluded, the user knows), 7 are inside the nested provider closure and are not loop exits, 4 are the provider-failure exits c8 fixed, 3 are compaction failures, and 1 -- the GraphExit::Failed arm at engine.rs:16553 -- was in NONE of the five buckets. 12+5+7+4+3+1 = 32. That thirty-second site is now c11; it is not a corner, and the census was the only thing asserting it did not exist -- all three the identical `prepare_durable_conversation / fire_on_session_end / cache_ledger.finish / save_session_mirror / return Err(e)` block. Those 3 said nothing on the answer stream. Fixed with the SAME shared helper and one shared wording, so a compaction exit and a provider exit cannot drift. RED ARM: deleting the three new call sites reddens a_compaction_bail_admits_itself_on_the_answer_stream at the assertion with the answer stream verbatim EMPTY (`: \"\"`), which is the measured symptom itself, while the control a_run_that_does_not_bail_carries_no_admission stays green (so the test cannot pass by admitting on every run) and c8 test stays green (so the mutation is scoped to this criterion). Restored, touched, re-run green."
  - id: c11
    text: "Expected-Behavior bullet 4 on the ORCHESTRATION-GRAPH exit: a run that ends because the turn's tool graph failed says so where the ANSWER went"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_graph_failure_exit_admits_itself_on_the_answer_stream"
    owner: core
    note: "CONCEDED to the round-2 verifier, in full: this exit was missed by c10's census and the census was the whole argument that nothing else remained. It is the same class by the same reasoning that produced c10 -- bullet 4 says only 'clearly mark the task as failed/incomplete', and bullet 7 names 'tool/runtime failure' as one of its five categories -- so it is closed with the same shared helper rather than argued away or filed. REACHABILITY, checked rather than assumed: the walker `tokio::spawn`s EVERY Node::AgentCall (orchestration/graph.rs, the AgentCall arm), and a JoinError from that task becomes GraphError::AgentFailed{agent:\"<join>\"} and lands on this arm. The dispatcher's `catch_unwind` covers `prepare_effect` and `execute` only, so a panic in any synchronous Tool trait method -- `is_concurrency_safe`, called by `partition()` at orchestration/mod.rs:697 inside `dispatch_once` inside that spawned task -- reaches the arm intact. The graph runs on every turn that has tool calls (the guard above it is `if tool_calls.is_empty()`), so this is the mainline dispatch path and not a corner. The arm called no emit at all: `emit_incomplete_run_admission` had 8 call sites and this was not one of them, and c8's live measurement already proved the run() wrapper adds nothing of its own to stdout, so a turn ending here wrote an EMPTY stdout with the cause on stderr. The admission is emitted FIRST, ahead of `prepare_durable_conversation()`, whose own `?` would otherwise carry the run out of the arm with nothing written; the Err text the caller sees is byte-identical to before. RED ARM: deleting the one new call reddens this test at the ADMISSION assertion (not at a precondition -- the preceding assertion, that the narration reached the stream, still passes), with the answer stream verbatim `\"let me look that up\"` -- the model's last narration standing alone, which IS the reported defect. Selective: in the same --no-fail-fast run c8's an_output_stall_stop_is_reachable_and_admits_itself, c10's a_compaction_bail_admits_itself_on_the_answer_stream and both controls stayed green. OVER-CORRECTION ARM, because a guard that only catches under-emission is half a guard: emitting the admission from the GraphExit::Continue arm instead reddens the control the_same_run_without_the_crash_carries_no_admission while the subject stays green, so the control is load-bearing and this test cannot pass by admitting on every run. Restored, touched, re-run green (5/5), tree byte-identical. SUPERSEDED AS A METHOD by c12: this was the third per-site fix in one session and the third census behind it, so the property is now enforced once in `run_inner` for every exit rather than argued exit by exit. The call site stays -- it carries a specific cause the backstop cannot derive -- but it is no longer the only thing standing between this exit and a silent stdout."

  - id: c12
    text: "Expected-Behavior bullet 4 as a CLASS: whatever terminal Err a turn ends on, the run says so where the ANSWER went -- enforced once for the whole run loop rather than once per exit"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::an_internal_authority_exit_admits_itself_on_the_answer_stream"
    owner: core
    note: "WHY A FOURTH BULLET-4 CRITERION EXISTS: c8, c10 and c11 each closed one bucket of run_inner_impl's terminal Err exits by editing the exit, and each rested on a census of `return Err(` sites asserting nothing else remained. That census was wrong twice in one session (29 -> 32), and the second correction came from a verifier rather than from this lane. Re-derived a third time at e4100643a, and it is not merely miscountable, it measures the wrong set: over run_inner_impl (engine.rs 12403-17180 at e4100643a; next fn at that indent is try_live_workflow, no nested fn definitions in the range) there are 32 `return Err(` lines AND 39 further lines carrying a `?` try-operator. Six of those sit at the function's own statement indent -- `let run_budget = self.current_run_budget()?;` at 12427 and `self.prepare_durable_conversation().await?;` at 17176, the last statement of the turn loop, among them -- and each ends run_inner_impl with Err exactly as a `return Err(` does. The other 33 sit inside nested blocks and closures and would each have to be classified one at a time, which is the point: the census never attempted it, so its total was never the exit count. A census of one set cannot certify a property of both, so no amount of care in counting could have closed bullet 4 as a class. Command, with its own control: `python3` over the file counting /[A-Za-z0-9_)\\]]\\?/ outside comment lines gives 39 in the run_inner_impl span, 0 in run_inner's own body (which has none) and 325 file-wide. THE EXCLUSION, DISPROVED BY MEASUREMENT RATHER THAN ARGUED AWAY: all three earlier criteria excluded the 12 SessionAuthority exits as 'internal faults, several in helper contexts, no answer stream guaranteed live'. A provider that streams one TextDelta and then an `LlmEvent::Error` carrying JOURNAL_AUTHORITY_ERROR_PREFIX ends the run at exactly such an exit through a full production `run()`. Run RED before any fix existed, the answer stream at that point was verbatim \"let me look that up\" -- the model's last narration standing alone, which IS the defect #388 reports. THE FIX is one backstop, not a 33rd call site: `emit_incomplete_run_admission` latches `admitted_incomplete_this_turn`; `run_inner` -- the wrapper every turn entry funnels through, which already resets and discloses `unserved_resends` the same way -- clears the latch at entry and, on Err, calls `admit_unspoken_run_failure`, which says the same sentence with a cause derived from the error variant only when nothing has said it yet. The eight exits that already admit keep their own, more specific cause. Its coverage is not a function of WHICH exit produced the Err -- it matches on the returned error at the wrapper -- so the `?` exits are covered by construction rather than by enumeration. That is the whole difference from c8/c10/c11. ONE DELIBERATE EXCLUSION, GRADED NOT ASSERTED: `AgentError::UserAborted`. The stop was the user's own, the mid-stream cancel arm already emits \"Run cancelled while receiving provider output.\", and the host is told through `RecoveryLifecycle::Cancelled` (wcore-cli/src/main.rs) rather than through an error. `a_cancelled_run_carries_no_admission` holds it, and its fixture cancels only AFTER it has observed the narration on the sink -- the first cut cancelled immediately, exited at an earlier cancel check with an empty answer stream, and would have passed for the wrong reason. THREE RED ARMS, each on a different line of the new code, tree committed at e4100643a first and touched after every edit and restore: (A) delete the `admit_unspoken_run_failure` call in `run_inner`: the subject reddens at the ADMISSION assertion and not at a precondition -- the preceding assertion, that the narration reached the stream, still passes -- with the answer stream verbatim \"let me look that up\", the model's last narration standing alone, which IS the reported defect. In the same --no-fail-fast run the other 9 stayed green, c8's an_output_stall_stop_is_reachable_and_admits_itself, c10's a_compaction_bail_admits_itself_on_the_answer_stream, c11's a_graph_failure_exit_admits_itself_on_the_answer_stream and all four controls among them, so the mutation is selective to this criterion; (B) delete the `if self.admitted_incomplete_this_turn { return; }` latch, the over-correction arm, because a guard that only catches under-emission is half a guard: an_exit_that_already_admitted_admits_exactly_once reddens with the answer stream carrying BOTH sentences -- 'the provider refused this request and it could not be repaired into one worth re-sending' followed by 'the provider call for this turn failed (API error 400: malformed request)' -- while the subject and the other 8 stay green. Asserting the COUNT rather than the presence is what catches it; (C) drop the UserAborted exclusion. Deleting the arm outright does NOT compile (E0004, `&AgentError::UserAborted` not covered), which is itself proof the line is live code rather than dead, so the mutation changes the DECISION instead of the syntax -- `AgentError::UserAborted => \"the run was aborted\".to_string()` -- and a_cancelled_run_carries_no_admission reddens with the stream verbatim \"let me look that up\\n\\n[stopped early] I did not finish this: the run was aborted. Anything above is partial work, not an answer.\", subject and other 8 green. All three arms restored with `git checkout --`, touched after both the mutation and the restore, and the tree re-verified byte-identical. RESIDUAL: graded on the engine and its sinks, not on the shipped binary. c8's live one-shot measurement covers the byte path from `emit_incomplete_run_admission` to stdout, and this criterion adds callers to that same helper rather than a second path, so the live leg is inherited rather than re-run."

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
run loop has four more terminal Err exits — the three compaction bails
and the orchestration-graph failure — and they were silent on the answer
stream. That is not a corner: "truncates because the
model runs out of token budget" is the reporter's own description of the
emergency bail. Closed with the same helper rather than filed, because they are
four call sites of a function that already existed and already had a live
proof behind it. The fourth (c11) was missed by c10's own census and conceded
to the verifier who recounted it: 32 Err exits, not 29. A census presented as
exhaustive is a claim like any other, and this one was graded by re-deriving
it, not by re-reading it.

Re-deriving it a third time ended the approach rather than the argument, and
that is c12. `return Err(` was never the exit set: the same function has 39
more lines carrying a `?`, six of them at its own statement indent, and those
end the turn with `Err` exactly as a `return Err(` does. No census of the
first set can certify a property of both, so bullet 4 is now
enforced once in `run_inner` -- whatever `Err` a turn ends on, if nothing has
said so on the answer stream, the wrapper does. The exclusion the three
earlier criteria leaned on went the same way: the twelve internal-authority
exits were said to have no live answer stream, and when that was measured
instead of read, the stream held the model's last narration and nothing else.
One abort case is still deliberately silent, and it is a test rather than a
sentence.

So the ticket now stays open on core (c7), not on flux.
