---
issue: 908
repo: FerroxLabs/wayland
kind: defect
title: "Bug report: reasoning tags leak into answers; sandbox child timed out; further sub-symptoms"
status: open
last_verified_commit: e7144c30a
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
    handoff: "FerroxLabs/wayland#1231"
    note: "CORRECTED 2026-08-29 at e7144c30a: this row was marked met while this file's own prose said c3 needs a reproduction before anyone grades it either way and Do not close this on c1 alone. The prose was right and the state was wrong. NEITHER half of reproduced and addressed survives. REPRODUCED: the cited test resolves and passes, and it is genuinely non-vacuous -- but its own doc comment labels it RED (candidate mechanism) and its stimulus is a hand-built LlmEvent::TextDelta(\"<think>I should answer this.</think>\") fixture, i.e. a mock of a HYPOTHESISED cause, not the reporter's stream. ADDRESSED: the reported sub-symptom is the reporter's own words, not producing any response at all. After the fix the user still gets no response -- engine.rs:15964-15980 (the raw_text_chars > 0 branch, emitted via emit_error so it does reach them) replaces a FALSE diagnosis with a TRUE one, and the message itself ends Ask again, or use a model that emits its answer outside its reasoning tags. The turn is also not committed to history (engine.rs ~:16000), so the conversation keeps no record either. Substituting the empty turn is diagnosed correctly for the user gets a response is the substituted-property failure this cycle has shipped repeatedly, so c3 is NOT met on the diagnosis alone. What core HAS earned here is real and is kept, and I re-verified it at this commit rather than inheriting the grade: `cargo nextest run -p wcore-agent --test issue_923_1109b_red_test` on hetzner-dsm -> `Summary [0.235s] 18 tests run: 18 passed, 0 skipped`. RED ARM, my own: `} else if raw_text_chars > 0 {` -> `} else if false && raw_text_chars > 0 {` at engine.rs:15964 (the mutated line printed back, so it landed on the branch CONDITION and not on the doc comment block beneath it), touched, rebuilt -> `Summary 18 tests run: 16 passed, 2 failed`, verbatim `panicked at crates/wcore-agent/tests/issue_923_1109b_red_test.rs:720:5: #908: the provider streamed a complete response and OUR filter removed it, and the user was told their endpoint may be incompatible`, with f_a_turn_of_only_stray_closing_tags_is_not_blamed_on_the_endpoint red alongside it at :758. Restored with git checkout -- + touch. So the diagnosis half is shipped, tested, non-vacuous, and controlled in both directions (f_control_a_provider_that_streams_nothing_still_gets_the_endpoint_diagnosis keeps the endpoint message alive for the truly-empty turn, f_control_reasoning_followed_by_an_answer_emits_no_error blocks an always-fires fix). The remainder -- the user gets an ANSWER, not an explanation of why there is none -- is FerroxLabs/wayland#1231, which carries a real-model reproduction requirement (a captured qwen3:8b-class stream, not a hand-authored TextDelta), a recovery/retry criterion, a history-commit criterion, the existing negative control as a must-stay-green, a separate criterion for the reporter's unmatched-closing-tag shape, and a Windows re-check for the 2026-08-29 recurrence on 0.13.6/0.13.9, both of which predate this fix."
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
