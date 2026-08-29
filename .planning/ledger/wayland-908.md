---
issue: 908
repo: FerroxLabs/wayland
title: "Bug report: reasoning tags leak into answers; sandbox child timed out; further sub-symptoms"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "Reasoning tags no longer leak into answers, history or hosts"
    state: met
    evidence: "commit:508405d4"
    owner: core
  - id: c2
    text: "The 'Sandbox child timed out' sub-symptom is addressed"
    state: not-met
    owner: core
    note: "same class as the Windows AppContainer sandbox work; not touched by 0.13.10"
  - id: c3
    text: "The remaining reported sub-symptom is reproduced and addressed"
    state: not-met
    owner: core
    note: "the report bundles several complaints under one title; a reporter follow-up on 2026-08-29 says the issue recurs, so this needs re-reproduction before it can be graded"
---

Partially fixed in v0.13.10. One of the three reported sub-symptoms — model
reasoning tags leaking into the visible answer, into stored history and out to
hosts — is fixed by `508405d4`.

This issue is a bundle, which is why it cannot be closed on one fix. It also
carries a fresh reporter comment (2026-08-29) saying the behaviour recurs on
Windows 11 Home, so c3 needs a reproduction before anyone grades it either
way. Do not close this on c1 alone.
