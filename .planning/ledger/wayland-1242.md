---
issue: 1242
repo: FerroxLabs/wayland
kind: defect
title: "Display-side reasoning filters still have no end-of-stream flush, so the screen and the stored turn disagree"
status: open
last_verified_commit: f72d97de
criteria:
  - id: c1
    text: "Each of the six display-side consumers either calls ReasoningFilter::finish() at its end-of-stream hook, or carries a comment at the construction site saying why it must not"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by the dur-reasoning lane while closing wayland#1221 and wayland#1222, as the DECOMPOSED remainder of those two. Nothing has been done. The six sites, verified present on this tree by grep for `ReasoningFilter`: crates/wcore-agent/src/output/terminal.rs:87, crates/wcore-agent/src/output/protocol_sink.rs:411, crates/wcore-agent/src/agents/channel_sink.rs:89 and :97, crates/wcore-cli/src/tui/app.rs:699, crates/wcore-cli/src/tui/protocol_bridge.rs:1925, crates/wcore-cli/src/acp_engine.rs:452. `finish` exists on the public surface (symbol:crates/wcore-types/src/reasoning_filter.rs::finish) and is called by exactly one consumer, the engine's history-side filter."
  - id: c2
    text: "With a provider streaming 'Use the <thinking> tag to wrap reasoning. Then answer.', the shown text and the stored assistant ContentBlock::Text are byte-identical, asserted in a test that reads both from one run"
    state: not-met
    owner: core
    note: "Filed 2026-08-30. Nothing has been done. This is the observable: after wayland#1221 the stored side is whole and the display side is still truncated to 'Use the ', so the two now disagree about the same turn. Before #1221 they agreed -- both were truncated -- which is why this only becomes gradeable now."
  - id: c3
    text: "The chosen wire shape for a post-stream flush on ProtocolSink is written down in docs/json-stream-protocol.md, and a host that ignores the new emission still renders a correct (if truncated) turn"
    state: not-met
    owner: core
    note: "Filed 2026-08-30. Nothing has been done. This is the reason the remainder is a separate ticket rather than one more line in the #1221 fix: the engine's flush appends to a String, but a sink's flush must EMIT after the stream is already over -- a late text_delta on a message the host may consider closed, a new event, or a correction to a thinking event already sent. ProtocolSink and protocol_bridge are a wire contract with the desktop app."
  - id: c4
    text: "A closed <think>...</think> block is still stripped from the display and still rendered as a Thought block -- wayland#908 c1 does not regress on any of the six"
    state: not-met
    owner: core
    note: "Filed 2026-08-30. Nothing has been done. The negative half: this is what stops 'flush everything' being taken as the fix. The equivalent control on the history side is crates/wcore-agent/tests/reasoning_eos_flush_test.rs::control_a_closed_reasoning_block_is_still_stripped_from_stored_history."
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
