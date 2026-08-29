---
issue: 908
repo: FerroxLabs/wayland
title: "Bug report: reasoning tags leak into answers; sandbox child timed out; further sub-symptoms"
status: open
last_verified_commit: 43848f75
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
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_923_1109b_red_test.rs::f_a_turn_the_reasoning_filter_emptied_is_not_blamed_on_the_endpoint"
    owner: core
    note: "engine.rs counts raw_text_chars BEFORE the reasoning filter and reports the true cause instead of blaming the endpoint. Siblings cover the reporter's five-bare-closing-tag shape plus two negative controls. LIMIT: this makes the empty turn HONEST, it does not restore an answer. The reporter's 2026-08-29 recurrence was on core 0.13.6 and 0.13.9, both before this fix."
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
