---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Output caps are decided from what is actually known about the served model, not from the alias the request named"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "Reasoner replay is decided the same way, from what is known rather than from the alias"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c3
    text: "A prompt silently discarded by an under-sized served window is named to the user rather than showing as low pressure"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "shipped under #1172; a different cause of the same reported symptom"
  - id: c4
    text: "The remaining four bullets of this ticket's own Expected Behavior list are met"
    state: blocked
    owner: flux
    note: "they are router-side: retry/stall behaviour inside the Free Models Router, which core cannot change from this repo. Ticket carries needs:flux"
---

Graded against this ticket's own Expected Behavior list: 3 of 7 bullets are
met at v0.13.10, which is why it stays open.

Core's half was that output caps and reasoner replay were being decided from
`request.model` — the alias the caller typed — rather than from the model the
router actually served. `0cab1cf8` decides both from what is known.

The rest is the Free Models Router's side of the same symptom and is owned by
the flux lane, not core. #1172 closed a third, independent cause of the same
user-visible complaint (an endpoint silently discarding the prompt), which is
worth reading alongside this before anyone re-grades it.
