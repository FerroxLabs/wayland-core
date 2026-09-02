---
issue: 1242
repo: FerroxLabs/wayland
kind: defect
title: "Display-side reasoning filters still have no end-of-stream flush, so the screen and the stored turn disagree"
status: closed
last_verified_commit: 8ace76d61
criteria:
  - id: c1
    text: "Each of the six display-side consumers either calls ReasoningFilter::finish() at its end-of-stream hook, or carries a comment at the construction site saying why it must not"
    state: met
    owner: core
    evidence: "file:crates/wcore-agent/src/output/protocol_sink.rs:495:fn drain_withheld_text"
    note: "All six call `finish()` at their end-of-stream hook; none needed the escape clause. Enumerated against the ticket's own list, each verified by grep with the count: (1) terminal.rs -- `emit_stream_end` drains and calls the new `TerminalSink::show`, extracted from `emit_text_delta` precisely so recovered text is not put back through `process` and swallowed a second time (`self.show(&recovered);`, terminal.rs:365, count 1); (2) protocol_sink.rs -- `drain_withheld_text` (:495, count 1), called from BOTH `emit_stream_end` and `emit_stream_end_full`, since both are live producer paths; (3)+(4) channel_sink.rs -- `drain_withheld_text` (:178, count 1), called on BOTH lanes (`reasoning` and `chunk_reasoning`) in `emit_stream_end`, each against its own msg_id; (5) tui/app.rs + protocol_bridge.rs -- the session filter is drained in the `StreamEnd` arm (`let recovered = app.session.reasoning_filter.finish();`, protocol_bridge.rs:314, count 1) and pushed into `session.streaming`, with a pointer added at the field's own doc; (6) protocol_bridge.rs:1925 replay -- one complete string rather than a stream, so the drain is unconditional (`visible.push_str(&filter.finish());`); (7) acp_engine.rs -- the `StreamEnd` arm queues the recovered text as a `TextDelta` AHEAD of the terminal (`let recovered = self.reasoning.finish();`, :578, count 1), which works because `poll_next` drains `pending` before it yields anything else. Ordering is the same everywhere and it matters: `finish` BEFORE the reasoning drain, because `finish` retracts the unclosed block's span from the capture buffer."
  - id: c2
    text: "With a provider streaming 'Use the <thinking> tag to wrap reasoning. Then answer.', the shown text and the stored assistant ContentBlock::Text are byte-identical, asserted in a test that reads both from one run"
    state: met
    owner: core
    evidence: "test:crates/wcore-agent/tests/issue_1242_display_flush_test.rs::wayland1242_c2_the_shown_text_and_the_stored_text_are_the_same_bytes"
    note: "One engine turn, one provider stream, both lanes read out of it: SHOWN is the concatenation of the `text_delta` frames a REAL `ProtocolSink` wrote (through the real serializer, over a recording emitter -- the display-side consumer that is also the wire contract with the desktop app), STORED is the assistant `ContentBlock::Text` off `engine.conversation_messages()`. The test asserts BOTH against the provider's own bytes as well as against each other, so a fix that made them agree by breaking both still fails. Covers the ticket's measured input plus #1222's two, and a second test covers the tag straddling a delta boundary, which a per-delta drain would pass the first and fail. One harness fact worth stating: `engine.run()` does not emit `stream_end` -- that is the caller's, and in production it is `wcore_cli::main` -- so the test emits it exactly as main.rs does. RED under M5, which disabled the ProtocolSink drain: exit 101, shown `\\\"Use the \\\"` against stored `\\\"Use the <thinking> tag to wrap reasoning. Then answer.\\\"` -- the ticket's own observable. Red arm exit codes captured DIRECTLY (`cmd > file 2>&1; $?`), never through a pipe; each mutation printed the enclosing function body before and after so it is proven to land on executable code, and each site was restored with `git checkout --` plus a `touch` so cargo could not serve the mutated binary. Log: /root/w-f13/s2-red2.log."
  - id: c3
    text: "The chosen wire shape for a post-stream flush on ProtocolSink is written down in docs/json-stream-protocol.md, and a host that ignores the new emission still renders a correct (if truncated) turn"
    state: met
    owner: core
    evidence: "file:docs/json-stream-protocol.md:357:#### End-of-stream `text_delta` (wayland#1242)"
    note: "The wire shape chosen is the one already in the contract: an ordinary `text_delta` on the in-flight msg_id, emitted BEFORE `stream_end`. The message is still open at that point, so a host that appends deltas in arrival order needs no change at all and simply receives the whole answer; a host that ignores it renders exactly what it renders today -- the same turn, truncated where the splitter stopped being sure -- which is the criterion's second clause. The doc says why a new event type or a correction to an already-sent `thinking` was rejected (a frame no host knows, on a message some hosts consider closed), and states what a host must not do. It also records a consequence found by measurement and NOT by design: an unclosed block now reaches the client TWICE, once as the `thinking` events streamed live while it was open and once as raw text in the end-of-stream `text_delta`. `finish`'s retraction can only reach what is still in the capture buffer, and a display consumer drains that after every chunk so reasoning streams live (#1129) -- by end of stream those bytes are already sent and the wire has no unsend. Duplicated is the honest end state; truncated was the one that cost the user the answer. Anchor verified unique with a control (`grep -cF` == 1)."
  - id: c4
    text: "A closed <think>...</think> block is still stripped from the display and still rendered as a Thought block -- wayland#908 c1 does not regress on any of the six"
    state: met
    owner: core
    evidence: "test:crates/wcore-agent/tests/issue_1242_display_flush_test.rs::wayland1242_c4_a_closed_reasoning_block_is_still_stripped_and_still_a_thought"
    note: "The inadmissible fix is excluded by test, not by intention. `wayland1242_c4_...` runs on the same harness as c2 and asserts that a CLOSED `<think>...</think>` block is absent from the shown answer, present as a `thinking` frame, and that stored agrees. RED under M6, which is the inadmissible fix itself -- `let visible = text.to_string();` in `ProtocolSink::emit_text_delta`, i.e. stop filtering on display: exit 101, `a closed reasoning block leaked into the answer the host is shown`, while c2's tests would have passed. Beyond ProtocolSink, the other five consumers keep their existing #908/#1129 coverage green: `issue_1129_reasoning_protocol_test` and `issue_1129_sub_agent_reasoning_test`, 18 tests, all green. THREE of those tests changed, and the change is a real behaviour change that must not be read as a regression: an UNCLOSED block now reaches the host on the TEXT lane rather than as a Thought. That is wayland#1222's already-shipped decision for the durable record ('a block that never closed was never a block') extended to the display, and it is what c2 requires -- the ticket itself calls the Thought-block treatment of that case 'misfiled'. #1129's property, that the content must not VANISH, is preserved; only the lane changed. A CLOSED block is untouched on all six. Red arm exit codes captured DIRECTLY (`cmd > file 2>&1; $?`), never through a pipe; each mutation printed the enclosing function body before and after so it is proven to land on executable code, and each site was restored with `git checkout --` plus a `touch` so cargo could not serve the mutated binary. Log: /root/w-f13/s2-red2.log."
---

Split out of wayland#1221 / wayland#1222 while closing them, so their remainder
is tracked rather than absorbed. Those two fixed the DURABLE record: the engine
now drains `ReasoningFilter::finish()` when the provider stream ends, so an
unclosed reasoning tag no longer eats the rest of the answer out of stored
history. Every display-side consumer of the same filter still calls only
`process` and `reset`.

Consequence on today's tree: a model that answers
`Use the <thinking> tag to wrap reasoning. Then answer.` stores that sentence
whole and shows the user `Use the `, with the remainder misfiled into a
collapsed Thought block.

This is strictly better than what #1221 found (where BOTH sides lost the text),
and no criterion of #1221 or #1222 depends on it -- both are written against
the durable record. It is filed because the divergence is real, user-visible,
and needs a decision this lane could not make on its own: what a sink emits
after the stream it was rendering has already ended.

Searched before filing: `reasoning filter`, `flush stream`, `reasoning tag
display`, `thought block`, `ProtocolSink reasoning`, `unclosed tag` and `1129`
across open issues in FerroxLabs/wayland and FerroxLabs/wayland-core. The only
hits were #908, #1221, #1222 and #1231 (an all-reasoning turn giving no answer
-- a different defect, on the empty-turn path). No existing carrier.
