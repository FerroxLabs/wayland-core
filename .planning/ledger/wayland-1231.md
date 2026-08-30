---
issue: 1231
repo: FerroxLabs/wayland
kind: defect
title: "An all-reasoning turn still gives the user no answer, only an accurate explanation of why there is none"
status: open
last_verified_commit: fe22fb34
criteria:
  - id: c1
    text: "REPRODUCED AGAINST A REAL MODEL: a captured stream from a model that emits its reasoning as ordinary inline text deltas over the OpenAI-compatible route, in which the filtered assistant text is empty while raw_text_chars > 0; the captured stream becomes the fixture. A hand-authored TextDelta does not satisfy this"
    state: not-met
    owner: core
    note: "Filed as its own tracker by the 0.13.12 close-sweep after wayland#908 c3 was REFUTED, and given a ledger file 2026-08-30 by lane/f13-w3-teardown. The file exists because the LIVE check-criteria-ledger run said `COVERAGE: FerroxLabs/wayland#1231 is OPEN and in scope with no ledger file` -- an open tracker with no ledger row is invisible to the release gate, and #908 c3 now hands its remainder here, so the handoff would have pointed at something nothing counts. Nothing has been built."
  - id: c2
    text: "THE USER GETS AN ANSWER: when assistant_text.is_empty() && tool_calls.is_empty() && raw_text_chars > 0, core recovers a usable reply rather than only emitting a diagnosis -- surface the captured reasoning content as a clearly-labelled answer, or take one automatic retry instructing the model to answer outside its reasoning tags"
    state: not-met
    owner: core
    note: "This is the half wayland#908 c3 substituted away: #908 shipped an honest diagnosis and the ledger`s own note conceded `LIMIT: this makes the empty turn HONEST, it does not restore an answer`. The reported sub-symptom is `not producing any response at all`, so a correct explanation is not the property. Which recovery to take is a design decision the issue says must be argued, not left open."
  - id: c3
    text: "THE CONVERSATION SURVIVES IT: the recovered answer is committed to history so the next turn has it"
    state: not-met
    owner: core
    note: "Today the empty turn is deliberately dropped, so the conversation carries no record of it. Gradeable only after c2 decides what a recovered answer is."
  - id: c4
    text: "NEGATIVE CONTROL: a turn that genuinely produced nothing (no raw text at all) still gets the honest empty-turn diagnosis and no fabricated answer"
    state: not-met
    owner: core
    note: "The control already exists and is green -- f_control_a_provider_that_streams_nothing_still_gets_the_endpoint_diagnosis in crates/wcore-agent/tests/issue_923_1109b_red_test.rs. It is graded not-met rather than met because the criterion is that it STAYS green across the c2 fix, and the c2 fix does not exist yet; marking it met today would grade a control against code it has never been exposed to. This is the row that blocks an always-fires recovery."
  - id: c5
    text: "THE STRAY-CLOSING-TAG SHAPE, SEPARATELY: establish by measurement whether the filter is correct to consume an unmatched closing tag with no opener; if a bare </thought> causes surviving answer text to be discarded, that is a filter defect distinct from c2 with its own fix and test"
    state: not-met
    owner: core
    note: "The reporter`s `just </thought> five times in a row` is an unmatched CLOSING tag with no opener, which is a different input class from the inline <think>...</think> block the #908 fix reasoned about. wayland#908 c1`s evidence now anchors stray_close_tag_is_stripped_from_conversation_history, which proves the tag is stripped from HISTORY -- it does not measure whether surrounding answer text survives, which is what this criterion asks."
  - id: c6
    text: "CHECKED WHERE IT WAS REPORTED: the reporter's 2026-08-29 recurrence is re-run on Windows against a build carrying c2"
    state: not-met
    owner: core
    note: "#908 c1 and c2 were both graded on Windows-adjacent evidence and this one must not be graded on Linux alone. SeanDesktop is the only Windows box. A green from a host that cannot exhibit the failure is not a pass."
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
