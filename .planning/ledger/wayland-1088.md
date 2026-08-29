---
issue: 1088
repo: FerroxLabs/wayland
kind: defect
title: "Bug report: Chat Interface Bug and Restricted Read / Glob / Write / Edit"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The typed event exists and the generated contract corpus row is correct"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/contract/generate.rs::generated_artifacts"
    owner: core
    note: "the guard now lives INSIDE generated_artifacts(), so a violating corpus cannot be emitted at all rather than being caught after the fact"
  - id: c2
    text: "The user-visible half — the chat interface no longer reports Read/Glob/Write/Edit as restricted"
    state: blocked
    owner: desktop
    handoff: "FerroxLabs/wayland#1223"
    note: "the surface that renders the restriction is Desktop's; core emits the typed event it needs. Ticket carries needs:desktop"
---

Partially fixed in v0.13.10.

Core's half was to emit a typed event the host can render correctly, and to
stop the generated contract corpus from carrying the wrong row. The guard was
moved inside `generated_artifacts()` so a violating corpus cannot be produced
in the first place — a check that runs after generation can be skipped; one
that runs inside it cannot.

What the reporter actually sees is rendered by Desktop, so this stays open
against that lane.
