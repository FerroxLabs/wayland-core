# Wayland Core F05 Capability Activation Receipt

**Status:** implementation and local Linux E3 seal passed; authoritative CI,
native-platform and Desktop-host seals pending

**Implementation source:** `0825c92d42fe1777822e2c3463f9eb581ba5cd5d` on
`frontier/m0`

**Scope:** F05 only. This receipt proves that each capability in the locked F05
set reports an honest startup state and that currently live capabilities emit
runtime proof only after a real side effect succeeds. It does not claim that
dormant capabilities have been activated.

## 1. Delivered contract

F05 now provides:

- stable typed identities for all eight audited capabilities;
- typed `declared`, `configured`, `constructed`, `ready`, `reached`,
  `outcome_changed`, `observed` and `unavailable` stages;
- typed unavailability reasons and transition validation;
- startup resolution from actual configuration and construction facts rather
  than advertised feature names;
- fail-closed effective settings when concrete memory construction fails;
- runtime outcome proof after successful smart-handoff persistence, procedure
  draft staging and legacy auto-skill drafting;
- JSON-stream and TUI delivery without changing the first JSON `ready` event
  or creating startup transcript/toast noise;
- a deterministic `CAPABILITIES` section in TUI `/doctor`; and
- an evaluator gate that requires valid startup proof for all eight identities
  at the existing frozen 1.0 honesty threshold.

The additive wire event and all serialized identifiers are documented in
`docs/json-stream-protocol.md`.

## 2. Startup truth table

| Capability | Effective startup truth | Runtime outcome proof |
|---|---|---|
| Pricing refresher | Unavailable: no production constructor | None |
| Mid-flight monitor | Unavailable: runtime path unwired | None |
| Cooldown tracker | Unavailable: no production constructor | None |
| Learned policy | Unavailable: runtime path unwired | None |
| Smart handoff | Disabled, dependency-unavailable, or ready from concrete memory construction | Successful episode persistence |
| Delegate isolation | Unavailable: isolation not enforced | None |
| Procedure skill drafting | Disabled, dependency-unavailable, or ready from concrete memory construction | Successful quarantine staging |
| Legacy auto-skill drafting | Disabled, dependency-/constructor-unavailable, or ready | Successful draft write |

An unavailable row is an honesty result, not capability completion. Pricing,
mid-flight monitoring, cooldown tracking, learned policy and Delegate isolation
remain product work for later frontier tasks.

## 3. Verification evidence

Verification ran in `/root/wayland-frontier-m0` on `hetzner-dsm` with Rust
1.95.0 at the exact source above:

- `cargo check -p wcore-agent -p wcore-cli`: passed;
- all five startup truth-table unit tests, including the independent audit's
  master-gate regression: passed;
- a real default `AgentBootstrap::build` regression proving the enabled handoff
  subflag cannot claim readiness while smart compaction is disabled: passed;
- smart-handoff success and persistence-failure outcome tests: passed;
- legacy auto-drafting outcome test: passed;
- the three-case procedure-drafting integration cohort: passed;
- JSON protocol startup delivery and TUI `/doctor` rendering tests: passed;
- all four evaluator chain/rate unit tests and three fixture-runner honesty
  tests, including rejection of `configured -> ready` without construction:
  passed;
- the packaged CLI startup-order test proving `ready` is the first JSON line and
  capability activation follows it: passed;
- the full `wcore-eval-scenarios` test command: passed, including 124 unit tests
  with one explicitly ignored live test and every integration test binary;
- the packaged deterministic OpenAI Core loop: passed; and
- strict clippy across `wcore-protocol`, `wcore-agent`, `wcore-cli` and
  `wcore-eval-scenarios`, all targets, with `-D warnings`: passed.

Local `cargo fmt --all` and `git diff --check` passed immediately before the
receipt commit.

## 4. Honesty gate behavior

The evaluator captures activation events during bootstrap, pre-command drain,
turn execution and final drain. It rejects a missing audited identity, malformed
reason, illegal transition, incomplete startup chain, event after terminal
unavailability, or incomplete runtime outcome cycle. A negative fixture that
omits one of eight identities produces a 0.875 honesty rate and fails the frozen
1.0 threshold.

The gate records its result through the existing assertion evidence in receipt
schema v1. Successful per-event activation rows are not yet retained as a new
receipt-body field; changing that representation requires an explicit schema
decision rather than silently mutating v1.

## 5. Independent audit verdict

The first read-only adversarial audit rejected the seal after proving that the
smart-handoff subflag could report readiness while its master smart-compaction
gate remained disabled. Commit `0825c92d42fe1777822e2c3463f9eb581ba5cd5d`
added the missing production input and regressions. The independent re-audit
returned PASS with no blocker or high finding and separately verified the real
bootstrap default, invalid configured-to-ready rejection, and production JSON
startup ordering.

## 6. Honest boundary and deferred gates

This is a local Linux E3 seal, not a cross-platform or Desktop product seal.

- ACP has no startup-ready capability projection and currently does not expose
  these events. ACP parity is required by F07's posture matrix.
- Wayland Desktop can consume the additive protocol event under the existing
  forward-compatibility contract, but its capability diagnostics UI has not
  been implemented or live-proven in this Core-only task.
- macOS and Windows native execution, authoritative CI provenance and the
  Desktop-hosted path remain pending by operator choice.
- F05 reports five unavailable capabilities honestly; it does not satisfy the
  later work needed to make them effective.

These boundaries do not block F07 implementation. They do block any claim that
all advertised Wayland capabilities are already frontier-complete.
