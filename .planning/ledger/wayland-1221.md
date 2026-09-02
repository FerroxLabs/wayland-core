---
issue: 1221
repo: FerroxLabs/wayland
kind: defect
title: "An assistant answer that merely mentions a reasoning-tag name in prose has everything after that word deleted from durable history"
status: closed
last_verified_commit: f72d97de
criteria:
  - id: c1
    text: "The input measured here -- 'Use the <thinking> tag to wrap reasoning. Then answer.' -- survives intact in the stored assistant text"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_eos_flush_test.rs::wayland1221_c1_prose_that_mentions_a_reasoning_tag_survives_intact_in_stored_history"
    owner: core
    note: "FIXED 2026-08-30. `ReasoningFilter::finish()` drains what the filter is holding back, and the engine calls it once the provider stream for the attempt is over (crates/wcore-agent/src/engine.rs, immediately before the AUDIT A3 attempt-outcome classification). For an unclosed open the drain returns the RAW bytes from the `<` of the opening tag onward -- the filter now keeps that raw copy in `raw_open` alongside the stripped and captured views -- so the ticket's measured sentence reaches `assistant_text` whole. The test asserts the stored assistant `ContentBlock::Text` read back off `conversation_messages()`, which is the same string that becomes the session mirror, the journal entry and the text replayed upstream. RED ARM: deleting the one statement `assistant_text.push_str(&assistant_reasoning.finish());` (the sole occurrence of that statement in engine.rs, executable) reds this test with `left: [\"Use the \"]` -- character-for-character the value the ticket measured."
  - id: c2
    text: "An unclosed reasoning tag does not silently eat the remainder of the DURABLE record: either the filter applies to display only, or an unclosed open is recovered at end of stream"
    state: met
    evidence: "symbol:crates/wcore-types/src/reasoning_filter.rs::finish"
    owner: core
    note: "CONSTRAINT ON ANY FUTURE FIX, and it is the reason this criterion reads as a disjunction: the DISPLAY-ONLY disjunct is INADMISSIBLE and must not be taken by anyone re-opening this. wayland#908 c1 is 'Reasoning tags no longer leak into answers, history OR HOSTS' and is already `met` on commit:508405d4; making the filter display-only un-meets its history clause. #908 c1's evidence is a commit hash, so it re-resolves green forever and NO gate would catch that regression -- the ledger would show two green criteria over a re-broken product. The only admissible disjunct is END-OF-STREAM RECOVERY, and that is what shipped: `ReasoningFilter::finish()` returns the raw bytes of an unclosed block and retracts the same span from the capture buffer so it is reported once, as text. A closed block is still stripped (that is #908 c1) -- see the `control_a_closed_reasoning_block_is_still_stripped_from_stored_history` sibling, which reds if the recovery is widened to cover closed blocks. RED ARM: neutralising the recovery branch in `finish` (`self.raw_open.take().filter(|_| false)`) reds 5 filter tests while leaving the `<`-prefix flush green, so the two halves of the drain are separately covered."
  - id: c3
    text: "If any content is dropped from stored history the user is told; the empty-turn notice at engine.rs:15941 is not the only guard"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_eos_flush_test.rs::wayland1221_c3_a_partial_strip_is_announced_to_the_user"
    owner: core
    note: "A SECOND guard, sitting on the `else` of the empty-turn notice: when the raw text the provider streamed is longer than what the filter let through, the turn says so through `emit_info` with the character count. The pre-existing guard can only fire when the strip is TOTAL (`assistant_text.is_empty()`), which is exactly why the ticket's shape escaped it. What is still strippable after the end-of-stream flush is a reasoning block that CLOSED, plus the bare tags -- deliberate under #908 c1, but deliberate is not announced, and the stripped body is what the provider replays on every later turn. Counted against `filtered_text_chars`, sampled right after the flush and NOT against the turn-final `assistant_text`: a Flux web_search turn also appends a grounding-sources block to `assistant_text` that never passed through the filter, and comparing the turn-final length would let that block mask a strip. Emitted through the output sink, not `tracing` -- RUST_LOG is unset on a default install and a `warn!` would reach nobody. TWO RED ARMS, both discriminating: disabling the branch reds this test alone; weakening its predicate to `>=` reds `control_a_turn_that_lost_nothing_announces_nothing` and `wayland1222_c4_controls_stay_byte_exact` and leaves this one green, so the guard cannot be satisfied by always firing."
  - id: c4
    text: "A test asserts the stored assistant ContentBlock::Text for that input; shown RED against today's engine.rs:14786"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_eos_flush_test.rs::wayland1221_an_unclosed_tag_split_across_deltas_is_recovered_whole"
    owner: core
    note: "The RED demonstration, run on this tree. The engine's flush call (the successor of the engine.rs:14786 site the ticket names -- 508405d4's `assistant_reasoning.process(&text)` is now at engine.rs:14798) was removed, the file touched so cargo could not serve a stale binary, and the suite re-run: 3 of the 7 tests failed -- twice with `left: [\"Use the \"]` (the c1 input and its split-delta twin) and once with `left: [\"the answer is 5 \"]`, reproducing the ticket's own measurements character for character; an earlier run of the same mutation, before the two #1222 c1 inputs were merged into one test, also produced `left: [\"result: \"]`. Restored with `git checkout --` and touched again. The cited test is the chunk-boundary twin of the c1 test: it splits `<thi` / `nking>` across two deltas, so a per-delta drain would pass c1's test and fail this one. Both read the stored assistant `ContentBlock::Text`, not the streamed copy."
---

FIXED 2026-08-30 on `lane/f13-dur-reasoning`, together with wayland#1222 -- they
are one change, because both are the same missing end-of-stream drain.

**What shipped.** `ReasoningFilter` gains `finish()`, a total drain for
everything `process` withholds, and the engine calls it once per attempt as
soon as the provider stream is over. Two things could be held back and each
now has a recorded decision:

* an undecided `<`-prefix (`the answer is 5 <`) -- the stream ended before it
  could become a tag, so it never was one; emitted verbatim.
* an OPEN reasoning block that never closed -- emitted verbatim as the raw
  bytes from its opening tag onward, with the same span retracted from the
  capture buffer so it is reported once, as text, and not twice.

A block that CLOSES is still stripped and still captured. That is #908 c1 and
it does not move.

**The constraint, written down so nobody takes the easy road later.** c2 offers
two disjuncts and only one of them is admissible. Making the filter
display-only would un-meet the history clause of #908 c1, whose evidence is a
commit hash and therefore re-resolves green forever -- the regression would be
invisible to every gate. See the c2 note above.

**Residual, split out rather than left partial.** The DISPLAY-side consumers of
the same filter (TerminalSink, ProtocolSink, ChannelSink, the TUI, the ACP
engine) still call only `process`/`reset`, so the screen still shows
`Use the ` where the stored turn now holds the whole sentence. Fixing that
needs a wire-shape decision for a post-stream emission on a protocol a host is
already reading, which is a different decision from this one; it is
FerroxLabs/wayland#1242 with its own acceptance criteria. No criterion of THIS
issue depends on it.
