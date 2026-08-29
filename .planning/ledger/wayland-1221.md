---
issue: 1221
repo: FerroxLabs/wayland
kind: defect
title: "An assistant answer that merely mentions a reasoning-tag name in prose has everything after that word deleted from durable history"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The input measured here -- 'Use the <thinking> tag to wrap reasoning. Then answer.' -- survives intact in the stored assistant text"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D41, found while verifying wayland#908). Nothing has been done. The measured finding, verbatim: The c1 fix extended a LOSSY display filter to the DURABLE conversation record, and an assistant answer that merely MENTIONS a reasoning-tag name in prose now has everything after that word silently deleted from stored history -- with no error, because the turn is not empty. Measured against the real crate: input 'Use the <thinking> tag to wrap reasoning. Then answer.' returns visible output 'Use the ' and files ' tag to wrap reasoning. Then answer.' into the reasoning capture buffer. `<thinking>` never closes, and the filter's documented behaviour is that an unclosed tag eats to end of stream. Before 508405d4 this cost the user a rendering; engine.rs:14786 now writes `assistant_reasoning.process(&text)` into `assistant_text`, which is the assistant ContentBlock::Text, the session mirror, the journal, and the text replayed upstream on the next request. So the truncation is permanent and travels back to the provider. The #908 empty-turn notice cannot catch it: engine.rs:15941 gates on `assistant_text.is_empty()`, and 'Use the ' is not empty. This is the most likely shape to bite in the field precisely because asking the agent about `<think>` tags, prompt formats, or this very bug is a normal thing to do."
  - id: c2
    text: "An unclosed reasoning tag does not silently eat the remainder of the DURABLE record: either the filter applies to display only, or an unclosed open is recovered at end of stream"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D41). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "If any content is dropped from stored history the user is told; the empty-turn notice at engine.rs:15941 is not the only guard"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D41). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "A test asserts the stored assistant ContentBlock::Text for that input; shown RED against today's engine.rs:14786"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D41). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The c1 fix extended a LOSSY display filter to the DURABLE conversation record, and an assistant answer that merely MENTIONS a reasoning-tag name in prose now has everything after that word silently deleted from stored history -- with no error, because the turn is not empty. Measured against the real crate: input 'Use the <thinking> tag to wrap reasoning. Then answer.' returns visible output 'Use the ' and files ' tag to wrap reasoning. Then answer.' into the reasoning capture buffer. `<thinking>` never closes, and the filter's documented behaviour is that an unclosed tag eats to end of stream. Before 508405d4 this cost the user a rendering; engine.rs:14786 now writes `assistant_reasoning.process(&text)` into `assistant_text`, which is the assistant ContentBlock::Text, the session mirror, the journal, and the text replayed upstream on the next request. So the truncation is permanent and travels back to the provider. The #908 empty-turn notice cannot catch it: engine.rs:15941 gates on `assistant_text.is_empty()`, and 'Use the ' is not empty. This is the most likely shape to bite in the field precisely because asking the agent about `<think>` tags, prompt formats, or this very bug is a normal thing to do.

**Where.** crates/wcore-agent/src/engine.rs:14786 (the history-side filter added by 508405d4), against the unclosed-open behaviour of crates/wcore-types/src/reasoning_filter.rs (FilterState::InThinking, documented at lines 44-46)

**Why it matters.** Silent, permanent, unannounced data loss in the durable conversation. The user sees a truncated answer with the remainder misfiled as a collapsed Thought block, the stored session keeps only the truncated half, and the truncated half is what the provider sees on every subsequent turn -- so the conversation degrades in a way neither the user nor a resumed session can detect or recover. It is also self-concealing: the one guard that would announce an over-strip only fires when the strip is total.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
