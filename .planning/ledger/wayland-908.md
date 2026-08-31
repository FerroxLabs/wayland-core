---
issue: 908
repo: FerroxLabs/wayland
kind: defect
title: "Bug report: reasoning tags leak into answers; sandbox child timed out; further sub-symptoms"
status: open
last_verified_commit: 6c87400b2
criteria:
  - id: c1
    text: "Reasoning tags no longer leak into answers, history or hosts"
    state: met
    evidence: "test:crates/wcore-agent/tests/reasoning_tag_history_test.rs::inline_reasoning_block_is_stripped_from_conversation_history"
    owner: core
    note: "EVIDENCE STRENGTHENED 2026-08-30, criterion text UNTOUCHED. It read `commit:508405d4`, and a commit hash is a record that a change was once made -- no gate replays it, so nothing in the tree would go red if the stripping regressed. The criterion now cites a STANDING test instead, which is the same property under a gate. HONEST COVERAGE MAP, because one anchor cannot carry three surfaces: `history` is the anchored file (inline_reasoning_block_is_stripped_from_conversation_history, stray_close_tag_is_stripped_from_conversation_history, reasoning_tag_split_across_deltas_is_stripped_from_history -- the reporter`s bare-closing-tag shape is the second of those); `hosts` is crates/wcore-agent/tests/issue_1129_reasoning_protocol_test.rs; `answers` and the sub-agent relay are crates/wcore-agent/tests/issue_1129_sub_agent_reasoning_test.rs, whose `tag_leak` helper is the shared oracle. NOT WEAKENED and deliberately not re-scoped: the sentence still claims all three surfaces, and the two unanchored surfaces are named here rather than dropped from the claim."
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
    handoff: "FerroxLabs/wayland#1231"
    note: "RE-CHECKED 2026-08-31 by lane `sandbox`, STATE UNCHANGED at `not-met`, and the reason is now a tree fact rather than an inference. The remainder lives on FerroxLabs/wayland#1231, and #1231 has since been WORKED -- its own comment of 2026-08-31 records c2 decided (SURFACE the captured reasoning, do not auto-retry), c1 captured against a real qwen3:8b stream, c5 REFUTED by measurement (a bare `</thought>` consumes exactly itself; no separate filter defect is owed) and c6 still open for want of a Windows box. NONE OF THAT IS IN THIS TREE. It is on branch `lane/f13-protocol`, and `git merge-base --is-ancestor lane/f13-protocol integ/f13` answers NOT-MERGED, so nothing on `integ/f13` -- or on this lane's branch, which is based on it -- restores an answer to the user. Grading c3 `met` from this lane would be grading a fix that is not in the tree being graded. Two further facts that hold independently of that merge: #1231 c6 requires the reporter's 2026-08-29 Windows 11 recurrence to be re-run on Windows against a build carrying c2, which no lane has done; and this is a SANDBOX lane, so the reasoning-filter remainder is not work this lane could have taken even had it been unowned. c2 of THIS ticket -- the `Sandbox child timed out` sub-symptom, the half that is this lane's area -- is untouched and stays met. THE 2026-08-30 NOTE FOLLOWS, UNALTERED: DECOMPOSED 2026-08-30, and the carrier was SEARCHED FOR rather than filed: `gh issue list -R FerroxLabs/wayland --state open --search \"no response empty turn reasoning\"` and `--search \"908 reasoning\"`, both run with a known-positive control in the same pass (`--search \"typed failure category ErrorInfo\"`, which returned #1237 and #1184, proving the query works). FerroxLabs/wayland#1231 -- `An all-reasoning turn still gives the user no answer, only an accurate explanation of why there is none` -- already exists, is OPEN, and is this exact remainder: it quotes the same `LIMIT: this makes the empty turn HONEST, it does not restore an answer` and undoes the same two substitutions (reproduced / addressed). So no ticket was filed. The state stays `not-met` rather than `blocked` because the owner is core and this ledger`s own gate refuses `blocked` owned by core (check-criteria-ledger.py:572-576, `core cannot block on itself`). The 2026-08-29 refutation below stands unaltered. PREVIOUS NOTE KEPT: engine.rs counts raw_text_chars BEFORE the reasoning filter and reports the true cause instead of blaming the endpoint. Siblings cover the reporter's five-bare-closing-tag shape plus two negative controls. LIMIT: this makes the empty turn HONEST, it does not restore an answer. The reporter's 2026-08-29 recurrence was on core 0.13.6 and 0.13.9, both before this fix. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: DOES NOT HOLD AS WRITTEN. The criterion is 'reproduced and addressed'; neither half survives inspection. (1) REPRODUCED: the cited test resolves (`crates/wcore-agent/tests/issue_923_1109b_red_test.rs:703`) and passes -- I ran the file, 18/18 green, and it is genuinely non-vacuous (`f_control_a_provider_that_streams_nothing_still_gets_the_endpoint_diagnosis` is a real positive control that keeps the endpoint message alive for the truly-empty turn, and `f_control_reasoning_followed_by_an_answer_emits_no_error` blocks an always-fires fix). But the test's own doc comment calls it a 'RED (candidate mechanism)' -- it is a mock-harness reproduction of a HYPOTHESIZED cause, not the reporter's. (2) ADDRESSED: the ledger's own note admits it: 'LIMIT: this makes the empty turn HONEST, it does not restore an answer.' The reported sub-symptom is 'not producing any response at all'; after the fix the user still gets no response, they now get an accurate explanation instead of a wrong one (engine.rs:15964-15980, `raw_text_chars > 0` branch, emitted via `emit_error` so it does reach the user). Substituting 'the empty turn is diagnosed correctly' for 'the user gets a response' is exactly the easier-property substitution the sweep has already been burned on twice. (3) The ledger's OWN PROSE contradicts its `state: met`: 'c3 needs a reproduction before anyone grades it either way' and 'Do not close this on c1 alone'. All three rows are nonetheless marked met."
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
