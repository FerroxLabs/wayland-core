---
issue: 1222
repo: FerroxLabs/wayland
kind: defect
title: "ReasoningFilter has no end-of-stream flush, so a pending ambiguous prefix is dropped from stored history"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "'the answer is 5 <' and 'result: <th' survive byte-exact through a completed turn"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D42, found while verifying wayland#908). Nothing has been done. The measured finding, verbatim: ReasoningFilter has no end-of-stream flush, so whatever sits in the ambiguous-prefix buffer when the stream ends is discarded rather than emitted as plain text. Its public surface is exactly `new`, `take_captured`, `take_captured_delta`, `process`, `reset` -- there is no `finish`/`flush`, and engine.rs calls only `process` and `reset`. Measured: 'the answer is 5 <' returns 'the answer is 5 ' (trailing `<` gone) and 'result: <th' returns 'result: ' (three characters gone). Both are now dropped from the durable history, not just the display. Controls from the same probe confirm the filter is otherwise narrow and this is specifically an end-of-stream artefact: 'if a < b then', 'if a <b then c' and '<div>hello</div>' all pass through byte-exact."
  - id: c2
    text: "The filter gains a flush/finish on its public surface and the engine calls it at turn end"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D42). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The decision for a pending InThinking buffer at end of stream is recorded rather than left implicit"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D42). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "The controls stay byte-exact: 'if a < b then', 'if a <b then c', '<div>hello</div>'"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D42). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

ReasoningFilter has no end-of-stream flush, so whatever sits in the ambiguous-prefix buffer when the stream ends is discarded rather than emitted as plain text. Its public surface is exactly `new`, `take_captured`, `take_captured_delta`, `process`, `reset` -- there is no `finish`/`flush`, and engine.rs calls only `process` and `reset`. Measured: 'the answer is 5 <' returns 'the answer is 5 ' (trailing `<` gone) and 'result: <th' returns 'result: ' (three characters gone). Both are now dropped from the durable history, not just the display. Controls from the same probe confirm the filter is otherwise narrow and this is specifically an end-of-stream artefact: 'if a < b then', 'if a <b then c' and '<div>hello</div>' all pass through byte-exact.

**Where.** crates/wcore-types/src/reasoning_filter.rs (no flush on the public API; FilterState::MaybeOpenTag pending buffer), consumed at crates/wcore-agent/src/engine.rs:14786

**Why it matters.** An answer that legitimately ends in `<` or a partial angle-bracket token loses characters from the stored record with no notice. Smaller blast radius than the defect above and it shares the same root cause (nothing drains the filter at turn end), so one flush call fixes both -- but it needs a decision about what a pending InThinking buffer should do, which is why it is worth filing rather than patching silently.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
