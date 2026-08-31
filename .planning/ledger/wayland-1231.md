---
issue: 1231
repo: FerroxLabs/wayland
kind: defect
title: "An all-reasoning turn still gives the user no answer, only an accurate explanation of why there is none"
status: open
last_verified_commit: 4251d68be
criteria:
  - id: c1
    text: "REPRODUCED AGAINST A REAL MODEL: a captured stream from a model that emits its reasoning as ordinary inline text deltas over the OpenAI-compatible route, in which the filtered assistant text is empty while raw_text_chars > 0; the captured stream becomes the fixture. A hand-authored TextDelta does not satisfy this"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1231_reasoning_recovery_test.rs::control_the_captured_stream_is_entirely_inside_reasoning_tags"
    owner: core
    note: "MET. A real stream captured on hetzner from `qwen3:8b` over a local Ollama's OpenAI-compatible route (`POST /v1/chat/completions`, stream=true, temperature=0), stored as `crates/wcore-agent/tests/fixtures/issue_1231_qwen3_all_reasoning.rs` (56 content deltas, 248 chars) beside the raw 48KB SSE it came from. `control_the_captured_stream_is_entirely_inside_reasoning_tags` asserts the fixture really is the class under test AND that the production filter empties it, so the criterion's precondition (filtered text empty while raw_text_chars > 0) is measured, not assumed. THE CAPTURE EARNED ITS KEEP: it showed that Ollama's OpenAI shim parses `<think>` OUT of content into a separate `reasoning` field (MEASURED: 797 reasoning chars, 0 content chars), so `<think>` cannot reproduce this over that route at all -- `<thought>`, the reporter's own tag, is the one that arrives inline. No hand-authored fixture would have found that."
  - id: c2
    text: "THE USER GETS AN ANSWER: when assistant_text.is_empty() && tool_calls.is_empty() && raw_text_chars > 0, core recovers a usable reply rather than only emitting a diagnosis -- surface the captured reasoning content as a clearly-labelled answer, or take one automatic retry instructing the model to answer outside its reasoning tags"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1231_reasoning_recovery_test.rs::an_all_reasoning_turn_gives_the_user_an_answer"
    owner: core
    note: "MET. The design decision is argued, not left open: SURFACE the captured reasoning as a clearly-labelled answer rather than take an automatic retry. Measured reason -- capturing the c1 fixture took four temperature-0 attempts to get one reply that honoured an explicit instruction about where to put its tags, so instruction-following on tag PLACEMENT is the unreliable thing here, and a retry asks that same unreliable behaviour to go the other way at the cost of a second billed round-trip and a second wait every time. Surfacing is deterministic and free, and the content is genuinely there: the model DID answer, our filter removed it. EVIDENCE: `an_all_reasoning_turn_gives_the_user_an_answer` replays the captured stream through a real `AgentEngine::run` and asserts the model's own answer text reaches the user's text stream under `REASONING_RECOVERY_LABEL`."
  - id: c3
    text: "THE CONVERSATION SURVIVES IT: the recovered answer is committed to history so the next turn has it"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1231_reasoning_recovery_test.rs::the_recovered_answer_is_committed_to_history"
    owner: core
    note: "MET. The recovered answer is pushed as a `ContentBlock::Text`, which makes `assistant_content` non-empty and so fires the existing commit -- the next turn, a resumed session and the next provider request all see it. EVIDENCE: `the_recovered_answer_is_committed_to_history` reads `engine.conversation_messages()` after a real run and asserts the answer is INSIDE the committed assistant message, not merely alongside a committed empty one."
  - id: c4
    text: "NEGATIVE CONTROL: a turn that genuinely produced nothing (no raw text at all) still gets the honest empty-turn diagnosis and no fabricated answer"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1231_reasoning_recovery_test.rs::control_a_turn_that_produced_nothing_gets_the_diagnosis_and_no_answer"
    owner: core
    note: "MET, and it is now graded against code it has actually been exposed to, which is why it was correctly not-met before. The recovery arm is inside `raw_text_chars > 0`, so a turn that produced nothing cannot reach it -- the control holds by construction, not by coincidence. EVIDENCE: `control_a_turn_that_produced_nothing_gets_the_diagnosis_and_no_answer` asserts the text stream is empty, no recovery label appears, and the honest empty-response diagnosis still fires. A second control, `control_an_ordinary_answer_is_not_labelled_as_recovered`, stops a change that labelled every reply as recovered from passing everything else. The pre-existing #908 control in issue_923_1109b_red_test.rs stays green."
  - id: c5
    text: "THE STRAY-CLOSING-TAG SHAPE, SEPARATELY: establish by measurement whether the filter is correct to consume an unmatched closing tag with no opener; if a bare </thought> causes surviving answer text to be discarded, that is a filter defect distinct from c2 with its own fix and test"
    state: met
    evidence: "test:crates/wcore-types/tests/issue_1231_stray_closing_tag_test.rs::a_stray_closing_tag_consumes_itself_and_nothing_around_it"
    owner: core
    note: "MET by MEASUREMENT, and the antecedent is FALSE: an unmatched closing tag consumes exactly itself and nothing around it. Measured through the production `ReasoningFilter` (process-per-delta then finish, as the engine does): `</thought>` -> ``; five bare closes -> ``; `Paris is the capital.</thought>` -> `Paris is the capital.`; `</thought>Paris is the capital.` -> `Paris is the capital.`; `Paris</thought> is the capital.` -> `Paris is the capital.`; and split across deltas (`Paris</th` + `ought> is the capital.`) -> `Paris is the capital.`. Controls in the same run: a MATCHED block is still stripped, untagged text is untouched, and the two differ. So there is NO separate filter defect and none is owed. What the measurement does explain is the reporter's symptom: five bare closes filter to the empty string, so a turn made of them is an EMPTY TURN -- c2's shape seen through different provider output, not a second bug. Guarded by `crates/wcore-types/tests/issue_1231_stray_closing_tag_test.rs` plus the engine-level `control_a_turn_of_only_stray_closing_tags_recovers_nothing`, which asserts the recovery does NOT fire there and the #908 diagnosis stands."
  - id: c6
    text: "CHECKED WHERE IT WAS REPORTED: the reporter's 2026-08-29 recurrence is re-run on Windows against a build carrying c2"
    state: not-met
    owner: core
    note: "NOT MET, and left not-met rather than graded on an adjacent property. The criterion is that the reporter's 2026-08-29 recurrence is re-run ON WINDOWS against a build carrying c2. Everything above was measured on Linux (hetzner). SeanDesktop is the only Windows box and this lane did not reach it, so nothing here is evidence for this row -- a green from a host that cannot exhibit the failure is not a pass, and #908 c1/c2 were already graded on Windows-adjacent evidence once. WHAT IS OWED, precisely: build this branch on Windows, run `cargo nextest run -p wcore-agent --test issue_1231_reasoning_recovery_test` there, and exercise the reporter's own case against a real model. NOTE ON DIFFICULTY, so the next lane does not re-derive it: nothing in the c2 fix is platform-conditional -- it is `take_captured()` plus a `ContentBlock::Text` push in the engine's turn loop, with no `#[cfg]` anywhere -- so the RISK this row is guarding is not that the fix behaves differently on Windows but that the reporter's environment produces a stream shape the Linux capture does not. That is what has to be checked, and it cannot be checked from here."
---

Split out of FerroxLabs/wayland#908 c3 after the 0.13.12 close-sweep REFUTED that
criterion. #908 c3 reads "The remaining reported sub-symptom is reproduced and
addressed", and neither half survived inspection: the cited evidence is a
hand-authored `TextDelta` fixture whose own doc comment calls it a "RED (candidate
mechanism)" -- a mock reproduction of a HYPOTHESISED cause, not the reporter's --
and every assertion in it is about the error STRING, none about the user receiving
an answer.

What shipped under #908 is real and is not being taken back: `engine.rs` counts
`raw_text_chars` BEFORE the reasoning filter and now tells the user the truth
instead of blaming the endpoint. That is a better failure, not a fix for this
report. The user still gets no response.

`#908` c1 (reasoning tags leaking) and c2 (the `Sandbox child timed out`
sub-symptom) are NOT in scope here and stay on `#908`.
