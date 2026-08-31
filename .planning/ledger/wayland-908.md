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
    state: superseded
    successor: FerroxLabs/wayland#1231
    owner: core
    handoff: "FerroxLabs/wayland#1231"
    note: "SUPERSEDED 2026-08-31 during the f13 landing pass. The remaining reported sub-symptom of #908 is an all-reasoning turn that gives the user no answer, and the carrier for it was SEARCHED FOR rather than filed: FerroxLabs/wayland#1231 already exists and is that exact remainder, quoting the same LIMIT: this makes the empty turn HONEST, it does not restore an answer. The search carried a known-positive control in the same pass, so the hit is not an artifact of a query that cannot fail. WHY SUPERSEDED AND NOT MET: lane f13-s2-relay-misc graded this met with #1231s test as the evidence, while its own note in the same entry read The state stays not-met -- a grade contradicting its own note is the shape that has had to be withdrawn on this release before. And met is the wrong encoding either way: this remainder is not discharged by #908s own work, it is CARRIED by another ticket, which this tree already has grammar for (wayland#1274 made successor a first-class FIELD precisely so prose could not decide where a residual lives). CHECKED, not assumed: #1231 is fully met in this tree, all six criteria, including c6 -- re-run on real Windows 10.0.26200.9168 against a live qwen3:8b over the OpenAI-compatible route, the reporters own case, with the recovered answer and the exact command quoted. So the remainder really is discharged; it is discharged somewhere else, and this entry now says where."
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
