---
issue: 908
repo: FerroxLabs/wayland
kind: defect
title: "Bug report: reasoning tags leak into answers; sandbox child timed out; further sub-symptoms"
status: open
last_verified_commit: cb2bf1a4
criteria:
  - id: c1
    text: "Reasoning tags no longer leak into answers, history or hosts"
    state: met
    evidence: "commit:508405d4"
    owner: core
  - id: c2
    text: "The 'Sandbox child timed out' sub-symptom is addressed"
    state: met
    evidence: "test:crates/wcore-sandbox/src/lib.rs::windows_default_selects_the_relaxed_job_object_backend"
    owner: core
    note: "ELIMINATION, not reproduction. The reporter ran 0.12.25; the Windows session default is now windows_job_object (asserted end-to-end at lib.rs:1118 too) and that backend contains no Timeout construction at all - every SandboxError::Timeout site is under backends/appcontainer/ (opt-in via WAYLAND_SANDBOX=appcontainer) or a non-Windows backend. No test drives the reporter's original 0.12.25 path."
  - id: c3
    text: "The remaining reported sub-symptom is reproduced and addressed"
    state: not-met
    evidence: "test:crates/wcore-agent/tests/issue_923_1109b_red_test.rs::f_a_turn_the_reasoning_filter_emptied_is_not_blamed_on_the_endpoint"
    owner: core
    note: "RE-GRADED 2026-08-29, down from met. The cited evidence is real and non-vacuous -- engine.rs counts raw_text_chars BEFORE the filter and names the true cause instead of blaming the endpoint, with two negative controls -- but the row graded itself against its own recorded LIMIT: `this makes the empty turn HONEST, it does not restore an answer`, while the reported sub-symptom is `not producing any response at all`. Diagnosing an empty turn correctly is a different property from the user getting their answer, and this file's own prose already said so (`c3 needs a reproduction before anyone grades it either way`). WHAT IS NOW CLOSED, as c4: a concrete engine-level mechanism for `no response at all` -- an inline reasoning tag that never closes eats the rest of the stream -- is reproduced end-to-end and the answer is restored to the DURABLE record. WHAT IS NOT, and is the precise remainder: the same unclosed block is still filed as reasoning on every DISPLAY surface, so the user reads their answer inside a collapsed Thought block instead of as the reply. Six consumers hold their own filter and their own end-of-stream policy -- output/protocol_sink.rs:411, output/terminal.rs:87, agents/channel_sink.rs:89 and :97, wcore-cli/src/acp_engine.rs:452, wcore-cli/src/tui/app.rs:699, wcore-cli/src/tui/protocol_bridge.rs:1925 -- and each already has a stream-end hook; the recovered text is always a SUFFIX of the stream, so appending `ReasoningFilter::finish()` there is order-correct on all six. Separately, the reporter\'s own trace is still unavailable, so nobody can assert this mechanism is the one they hit."
  - id: c4
    text: "The history-side reasoning filter cannot delete text from the durable conversation record"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_tag_history_test.rs::a_prose_mention_of_a_reasoning_tag_survives_in_history"
    owner: core
    note: "Added 2026-08-29. c1 extended a DISPLAY filter to the durable record, and the filter is lossy by design: an opening tag that never closes eats to end of stream (the deliberate v0.9.0 choice) and an ambiguous `<` prefix that never resolves goes with it. engine.rs:14786 writes that filter output into assistant_text, which IS the assistant ContentBlock::Text, the session mirror, the journal, and the text replayed upstream next turn. REPRODUCED through the real engine on the unmodified tree, not modelled: input `Use the <thinking> tag to wrap reasoning. Then answer.` stored as `Use the `; `the answer is 5 <` stored as `the answer is 5 `; `result: <th` stored as `result: `. Silent -- engine.rs:15941 gates the empty-turn notice on assistant_text.is_empty() and the surviving prefix is not empty. FIXED by ReasoningFilter::finish(), a separate end-of-stream drain that reclassifies an unclosed block as the plain text it turned out to be (returning the opening tag and body verbatim, including swallowed nested-tag text, and retracting the body from the capture buffer) and flushes an unresolved prefix; wired ONLY into the durable path, so every display consumer keeps its own end-of-stream policy. RED ARMS: deleting the engine wiring line reddens 3 of 7 in reasoning_tag_history_test; gutting finish() reddens 5 of 43 reasoning_filter unit tests. CONTROLS that pass on BOTH arms: a_closed_reasoning_block_is_still_removed_from_history and finish_does_not_resurrect_a_block_that_closed. ONE DELIBERATE REMAINING DROP, pinned by finish_does_not_resurrect_a_stray_closing_tag: a recognised close with no matching open is still removed, because that is this ticket own reported symptom."
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
