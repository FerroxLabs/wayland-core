---
issue: 1222
repo: FerroxLabs/wayland
kind: defect
title: "ReasoningFilter has no end-of-stream flush, so a pending ambiguous prefix is dropped from stored history"
status: closed
last_verified_commit: f72d97de
criteria:
  - id: c1
    text: "'the answer is 5 <' and 'result: <th' survive byte-exact through a completed turn"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_eos_flush_test.rs::wayland1222_c1_both_measured_inputs_survive_byte_exact"
    owner: core
    note: "Both measured inputs in ONE test, so the criterion has ONE anchor rather than a split one. 'Through a completed turn' is taken literally: the test drives the real engine with a mock provider through `LlmEvent::Done` and reads the stored assistant `ContentBlock::Text` back off `conversation_messages()` -- not the filter in isolation, and not the streamed copy. RED ARM: deleting the engine's single flush statement (the sole flush statement in engine.rs, executable) reds it. The test loops both inputs and stops at the first, so the failure prints `left: [\"the answer is 5 \"]`; an earlier run of the same mutation against the two inputs as separate tests printed `left: [\"result: \"]` as well. Both are the ticket's measurements verbatim. Filter-level siblings for the same two inputs live in crates/wcore-types/tests/reasoning_filter_eos_flush.rs."
  - id: c2
    text: "The filter gains a flush/finish on its public surface and the engine calls it at turn end"
    state: met
    evidence: "symbol:crates/wcore-types/src/reasoning_filter.rs::finish"
    owner: core
    note: "`pub fn finish(&mut self) -> String` joins `new`/`take_captured`/`take_captured_delta`/`process`/`reset` on the public surface. The engine call is one statement, `assistant_text.push_str(&assistant_reasoning.finish());`, placed after the provider receive loop and BEFORE the attempt-outcome classification, so it runs once per attempt and a retry's `assistant_text.clear()` + `assistant_reasoning.reset()` cannot carry a failed attempt's flush forward. `finish` is idempotent and does not reset the filter for a new stream. The engine-calls-it half of this sentence is proven behaviourally by the c1 red arm above: removing that one statement reds four tests. Deliberately NOT wired into the six display-side consumers -- that needs a wire-shape decision on a live host protocol and is FerroxLabs/wayland#1242."
  - id: c3
    text: "The decision for a pending InThinking buffer at end of stream is recorded rather than left implicit"
    state: met
    evidence: "file:crates/wcore-types/src/reasoning_filter.rs:61:taken explicitly rather than left implicit"
    owner: core
    note: "Positional on purpose: the record IS a block of module documentation, at the 'End-of-stream decision (FerroxLabs/wayland#1221, #1222)' heading on line 48 of crates/wcore-types/src/reasoning_filter.rs, restated on `finish`'s own doc comment. THE DECISION, verbatim: a pending `InThinking` buffer at end of stream is RECOVERED, not dropped and not kept as reasoning -- the filter emits the raw bytes consumed since (and including) the opening tag as ordinary text, and retracts the same span from the capture buffer. The reasoning, also recorded there: a block that never closed was never observed to be a reasoning block, only prose containing a tag-shaped word; recovery is biased toward KEEPING text, because a visible runaway reasoning tail is recoverable and a deleted answer is not. The same block records that the display-only alternative is inadmissible (it un-meets wayland#908 c1's history clause). The stale `v0.9.0` line that claimed an unclosed tag 'eats to the end of the stream' -- the sentence #1221 quotes as the documented behaviour -- was corrected in the same edit rather than left to contradict the new one. RE-ANCHORED 2026-08-30 during the integ/f13 merge of lane/f13-keystone-1198, which closed FerroxLabs/wayland#1198 and made a bare file:<path>:<line> a REFUSED evidence token -- it only ever asserted that the file was that long, so any number under its length passed forever. This entry carried the bare reasoning_filter.rs:48, the module-doc heading `End-of-stream decision (FerroxLabs/wayland#1221, #1222)`. It is now anchored 13 lines down on the sentence that IS this criterion restated in the source -- `the decision it encodes -- taken explicitly rather than left implicit, per wayland#1222 c3 -- is:` -- because `rather than left implicit` is the half of the criterion a section heading cannot prove. The InThinking arm of that recorded decision is the third bullet under it (reasoning_filter.rs:66, the OPEN-block RECOVER case). Content re-anchor only; the criterion was not re-graded and no code moved."
  - id: c4
    text: "The controls stay byte-exact: 'if a < b then', 'if a <b then c', '<div>hello</div>'"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_eos_flush_test.rs::wayland1222_c4_controls_stay_byte_exact"
    owner: core
    note: "All three controls through a completed turn, asserted on the stored assistant text, and each also asserted to emit no partial-strip notice -- so a flush that over-emits (double-writing the pending buffer, or re-emitting an already-resolved tag) reds here instead of shipping. These pass on the UNFIXED tree too, which is the point: they are the negative half. They are not vacuous -- weakening the c3 notice predicate to `>=` reds this test, proving it observes the notice channel, and the filter-level twin `control_the_narrow_cases_stay_byte_exact` reds under a mutation that makes `finish` return the pending buffer twice."
---

FIXED 2026-08-30 on `lane/f13-dur-reasoning`, together with wayland#1221 -- one
change, because both tickets are the same missing end-of-stream drain. The
prose fix is described in `.planning/ledger/wayland-1221.md`; this entry
records the flush itself.

**The decision this ticket asked for.** #1222's own text says the flush "needs
a decision about what a pending InThinking buffer should do, which is why it is
worth filing rather than patching silently." The decision taken, and now
recorded in the module docs at reasoning_filter.rs:48 rather than left implicit
in the code:

* pending `MaybeOpenTag` -> emit verbatim. The stream ended before the prefix
  could become a tag, so it never was one.
* pending `InThinking` (an open block that never closed) -> RECOVER verbatim,
  from the `<` of the opening tag onward, and retract the same span from the
  capture buffer so it is not also reported as reasoning.

Recovery is deliberately biased toward keeping text. A runaway reasoning tail a
provider truly failed to close is now visible in history rather than silently
deleted -- that is the trade #1221 asks for, because a visible artefact is
recoverable and a deleted answer is not.

**Not in scope, and split out rather than left partial.** The six display-side
consumers of the same filter still call only `process`/`reset`; wiring them
needs a wire-shape decision for a post-stream emission on the JSON stream
protocol. That is FerroxLabs/wayland#1242. c2 as written asks for the engine,
and the engine is done.
