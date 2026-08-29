---
issue: 434
repo: FerroxLabs/wayland
title: "Flux tier-alias -> strict-reasoner: #417 replay gap (engine keys off request.model, alias resolves server-side)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The replay socket is populated, so the engine has somewhere to learn the served model from"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "The gap is closed for the turn on which the alias first resolves, not only for turn N+1"
    state: not-met
    owner: core
    note: "by construction the socket covers only N+1: the engine cannot know the server-side resolution before the first response arrives"
  - id: c3
    text: "The alias-resolves-server-side path is closed end to end"
    state: blocked
    owner: flux
    note: "requires the router to declare the resolved model on the turn it resolves it; core cannot close this alone. Ticket carries needs:flux"
---

Partially fixed in v0.13.10. The engine keys reasoner replay off
`request.model`, but a Flux tier alias resolves to a concrete strict-reasoner
server-side, so the engine is deciding on a name that is not the model.

The replay socket now exists and is populated, which is core's half. It
covers turn N+1 by construction — there is nothing to learn from until the
first response comes back — so a single-turn run still gets the alias
behaviour. Closing that requires the router to say what it resolved, which is
the flux lane's change, not core's.
