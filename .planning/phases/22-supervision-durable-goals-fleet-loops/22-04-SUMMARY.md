---
phase: 22-supervision-durable-goals-fleet-loops
plan: "04"
subsystem: surfaces
tags: [cli, tui, host-protocol, contract, not-run]
requires:
  - "22-01"
  - "22-02"
  - "22-03"
provides:
  - a fenced Desktop wire-contract seam request for the orchestrator to serialize
affects: []
tech-stack:
  added: []
  patterns: []
key-files:
  created: []
  modified: []
decisions:
  - "Not executed: one hard lane boundary and two unmet dependencies, none of which this lane may resolve unilaterally"
metrics:
  duration: not executed
  completed: 2026-07-27
status: not-run
---

# Phase 22 Plan 04: Goal Surfaces and Bounded Loops — Summary

**NOT RUN. No file in this plan's `files_modified` was touched.**

This is a refusal with reasons, not an omission. Three independent blockers, any
one of which is sufficient.

## Blocker 1 — Task 2 is outside this lane's authority (hard boundary)

Task 2 regenerates the Desktop wire-contract corpus: it changes
`crates/wcore-protocol/src/contract/generate.rs`, bumps `CONTRACT_MINOR` and
`GENERATOR_VERSION`, and rewrites
`crates/wcore-protocol/contracts/desktop/v1/manifest.json` by running
`wcore-contract generate`.

The lane brief forbids exactly this:

> **Do NOT run `wcore-contract generate`.** Regenerating Desktop wire-contract
> fixtures is a release-coordination action. If a plan needs a contract change,
> write a fenced seam request into your SUMMARY instead.

The seam request is below, as instructed. Note also that the plan's own Task 2
action text records that this artifact "conflicts deterministically" — which is
precisely why it is serialized by the orchestrator rather than done inside a
parallel lane.

## Blocker 2 — the foundation does not exist

Every surface this plan adapts is missing from the tree:

```
MISSING crates/wcore-agent/src/goal/loop_policy.rs
MISSING crates/wcore-agent/src/slash/goal.rs
MISSING crates/wcore-cli/src/goal_cmd.rs
MISSING crates/wcore-cli/src/tui/surfaces/goals.rs
```

`crates/wcore-agent/src/goal/` does not exist at all. 22-01 shipped the Goal
vocabulary into `wcore-types::goal` but never built the kernel, the
`SessionEvent` variants, the reducer arm or the `ReducedSessionState` field — its
own SUMMARY says so. Task 1's premise is "one typed Core command and event set
with three thin adapters over it"; there is no Core state for an adapter to be
thin over, so what would actually get built is a fourth parallel lifecycle — the
exact thing this plan's own truths forbid.

## Blocker 3 — its wave-3 dependency is not complete

`depends_on: ["22-01", "22-02", "22-03"]`. 22-03 is `status: partial` — Task 1
delivered, Tasks 2–4 not run. 22-01 and 22-02 are both `status: partial` as well.
Task 3 of this plan asks for one real Goal driven through the real binary and
observed identically from three surfaces; there is no Goal to drive.

## What was NOT done

Nothing in this plan. No typed command or event set, no loop policy enforcement,
no slash command, no CLI subcommand, no TUI surface, no contract fixtures, no
parity capture, no suspend/resume/preempt/reconnect exercise, no panel.
`22-04-SURFACE-PARITY.md` and `22-04-EVIDENCE/` were not created.

**F22-01, F22-05 and F22-07 are untouched. Success Criteria 1 and 4 remain
FAILED**, exactly as `22-PHASE-VERDICT.md` already grades them.

---

## Fenced seam request — Desktop wire contract

Per the brief, recorded here rather than performed. **No lane should action this
in parallel; it conflicts deterministically.**

```seam-request
ARTIFACT:   crates/wcore-protocol/contracts/desktop/v1/  (manifest.json + fixture trees)
GENERATOR:  crates/wcore-protocol/src/contract/generate.rs
REQUESTED:  additive minor bump adding Goal / task / wait / log / cursor fixtures
BLOCKED-BY: lane brief - "Do NOT run wcore-contract generate"

MEASURED BASE (read from the tree at lane/22, 2026-07-27):
  CONTRACT_MAJOR    = 1
  CONTRACT_MINOR    = 8
  GENERATOR_VERSION = wcore-desktop-contract-gen/11

REQUESTED CHANGE (from 22-04 Task 2):
  - bump CONTRACT_MINOR 8 -> 9, exactly once, additive; do NOT bump the major
  - move GENERATOR_VERSION alongside it in the existing style
  - emit a fixture for each Goal lifecycle command; for each Goal, task, wait,
    log and cursor event; and for the terminal transition of EVERY category in
    the 22-01 taxonomy (wcore_types::goal::GoalTerminalState) - including the
    easily-missed Unpriced, PartiallyCompleted, NeedsEscalation and
    AuthorityUnreconstructable ones
  - extend the adversarial tree with: stale cursor, duplicate acknowledgement,
    unknown field on a closed Goal payload, command naming an unknown Goal -
    each asserting WHICH refusal, not merely that one occurred
  - compatibility tree must prove a consumer pinned to minor 8 still decodes

PRECONDITION THE ORCHESTRATOR MUST CHECK FIRST:
  This request is NOT actionable yet. The fixtures serialize Goal state that
  does not exist: crates/wcore-agent/src/goal/ is absent and no Goal
  SessionEvent variants are defined. Generating a corpus for an unimplemented
  producer would publish a contract nothing can emit. Sequence the 22-01 kernel
  first, then this.
```

---

## Honest verdict

This plan's criteria were not met and were not attempted. Given the state of the
tree, attempting it would have produced a CLI surface, a TUI surface and a wire
contract over a Goal lifecycle that does not exist — three adapters over nothing,
plus a published contract for an unimplementable producer. That is worse than
zero, and it is the "parallel lifecycle" the phase brief explicitly forbids.

The correct next action is not this plan. It is 22-01 Task 3: build the kernel,
the `SessionEvent` variants, the reducer arm and the `ReducedSessionState` field
(which must be `#[serde(default, skip_serializing_if = ...)]` or 22-01's measured
journal byte-identity property stops holding). Everything in waves 3 and 4 is
downstream of that one missing piece.
