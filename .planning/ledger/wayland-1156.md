---
issue: 1156
repo: FerroxLabs/wayland
title: "[Bug]: acp serve survives its parent and reparents to PPID 1 — 9 orphans found, oldest 24h, pinning 160GB"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "acp serve profile children are bound to a parent-death channel, so the product does not leave the reported orphans"
    state: met
    evidence: "symbol:crates/wcore-cli/src/parent_channel.rs::watch_for_orphaning"
    owner: core
    note: "shipped and live-proven"
  - id: c2
    text: "The TEST SUPERVISOR owns the tree — no test site spawns acp serve unbound"
    state: not-met
    owner: core
    note: "five test sites still spawn acp serve unbound, including profile_router_live.rs:99-115 which spawns the supervisor itself with Stdio::null(). This is what the ticket asked for"
---

The product half is fixed in v0.13.10; the half this ticket asked for was
not, and the distinction matters.

The evidence in the report was a surviving SERVER with a surviving child. The
fix kills the child. Nothing kills the server. Five test sites still spawn
`acp serve` unbound, so the same nine-orphan pile-up is still reachable from
the test suite on the build host.

Grading c1 as "fixed" and closing would be exactly the substitution — a real
fix, to an adjacent problem, presented as the reported one.
