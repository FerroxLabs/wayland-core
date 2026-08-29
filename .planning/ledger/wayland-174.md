---
issue: 174
repo: FerroxLabs/wayland
title: "[Feature]: prevent runaway token spend with budget guards and spend audits"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Budget presets expand into concrete caps that reach the engine"
    state: met
    evidence: "test:crates/wcore-config/tests/budget_preset_test.rs::tiny_preset_reaches_the_engine_as_tiny_limits"
    owner: core
    note: "the sibling large-preset test in the same file pins the other end of the range"
  - id: c2
    text: "A per-task spend audit record is produced after every task"
    state: not-met
    owner: core
    note: "no spend_audit surface exists in the tree; the ledger holds the data and nothing emits the report"
  - id: c3
    text: "A no-paid-models mode exists and is enforced"
    state: not-met
    owner: core
    note: "no_paid resolves to nothing anywhere in crates/"
  - id: c4
    text: "A local-only mode exists and is enforced"
    state: not-met
    owner: core
    note: "local_only resolves to nothing anywhere in crates/"
  - id: c5
    text: "Silent model escalation is blocked and every escalation reason is durably recorded"
    state: not-met
    owner: core
    note: "escalation is modelled for child attribution only — there is no gate that blocks one and no durable record of its reason"
---

The canonical Token Spend Governance tracking issue. It asks for per-task,
per-agent and per-model budgets with soft warnings, hard stops, an escalation
approval gate, a retry circuit breaker, a live spend meter and a per-task spend
audit, plus presets including local-only and no-paid-models.

A large part landed. The engine mechanisms shipped in 0.12.6 — the routing-tier
swap that actually dispatches and bills the cheap model, cheap and
usage-accurate compaction, a bounded retry re-bill, cache hygiene — after a
ten-angle investigation that also refuted several alarmist candidates. Presets
reach the engine, usage is visible during a run and the numbers on the paths
that used to lie are now true, and repeated provider failures trip a circuit
breaker in `wcore-providers`.

Four of the issue's eleven acceptance bullets are open, and the criteria above
are those four plus the one landed item they are most easily confused with. Each
was checked by name against the tree rather than taken from the status comment:
`spend_audit`, `no_paid` and `local_only` return nothing. The remaining work
splits cleanly into the audit surface, the two modes, and the escalation gate.
