---
phase: 22-supervision-durable-goals-fleet-loops
plan: "04"
subsystem: surfaces
tags: [cli, tui, host-protocol, contract, partial]
requires:
  - "22-01"
  - "22-02"
  - "22-03"
provides:
  - a user-reachable CLI Goal surface (open / task / run / status / exec-task / effects)
  - the canonical JSON projection three surfaces can agree on, emitted from reduced state
  - a Desktop wire-contract seam request whose blocking precondition is now SATISFIED
affects:
  - crates/wcore-cli
tech-stack:
  added: []
  patterns:
    - "the status projection IS the reduced state, never a hand-built view of it"
key-files:
  created:
    - crates/wcore-cli/src/goal_cmd.rs
  modified:
    - crates/wcore-cli/src/lib.rs
    - crates/wcore-cli/src/main.rs
decisions:
  - "One of three surfaces delivered; TUI and host protocol are named as not done rather than stubbed"
  - "The contract seam request is updated, not regenerated: `wcore-contract generate` remains a release-coordination action reserved to the orchestrator"
metrics:
  duration: one session
  completed: 2026-07-27
status: partial
---

# Phase 22 Plan 04: Goal Surfaces and Bounded Loops — Summary

**Supersedes the previous `NOT RUN` grade.** That refusal was correct at the
time: all three of its blockers were real, and building three adapters over a
Goal lifecycle that did not exist would have been worse than zero. Two of the
three are now gone. The third is unchanged and still binding.

| Blocker as recorded | Status now |
|---|---|
| 1 — Task 2 regenerates the Desktop wire contract; the lane brief forbids `wcore-contract generate` | **UNCHANGED and still binding.** Seam request below, updated. |
| 2 — the foundation does not exist (`crates/wcore-agent/src/goal/` absent) | **GONE.** The kernel, the ledger, the `SessionEvent` variants, the reducer arm and the `ReducedSessionState` field all exist and are live-proven. |
| 3 — the wave-3 dependency (22-03) is incomplete | **SUBSTANTIALLY GONE.** 22-03's ledger is built, wired and proven on the shipped binary on Linux. |

## What landed

`crates/wcore-cli/src/goal_cmd.rs` — `wayland-core goal`, six verbs:

| Verb | What it does |
|---|---|
| `open` | authorize a durable Goal with a loop bound and a limit envelope |
| `task` | declare a task, its dependency set and its idempotency key |
| `run` | recover, revoke expired claim leases, drain the outbox, then drive waves through the real `FleetDispatcher` |
| `status` | the canonical JSON projection of Goal + task state, replayed from the chain |
| `exec-task` | the effect boundary — the atomic idempotency gate, then the operator's command |
| `effects` | count the effects on disk, with `--expect N` so a proof has a gate that can go red |

Two choices worth naming:

**The `status` projection IS the reduced state**, serialized directly, not a
hand-built view assembled beside it. A surface that renders its own shape is a
surface that can disagree with the chain, and "three surfaces observe identical
state" cannot be built out of three things that each decide what to show. This is
the shape a TUI and a host adapter should consume unchanged.

**`--iterations` cannot spell "unbounded."** The canonical taxonomy has no such
variant, `1` records `LoopPolicy::Once` and anything higher records `Fixed`, and
the bound is enforced by the reducer at the durable boundary — pinned by
`the_authorized_loop_bound_stops_the_run_even_though_work_remains`, which runs a
four-deep dependency chain against a two-iteration authorization and stops with
real work outstanding. A flag that invented an unbounded variant would be a
second loop vocabulary beside the canonical one, which is the parallel lifecycle
this phase exists to remove.

## Criterion 1 — still FAILED, and now for a smaller reason

> CLI, TUI, and host-protocol paths observe and control identical Goal, child,
> task, wait, log, cursor, and terminal producer state, and emit the canonical
> serialized producer fixtures consumed later at D2.

**FAILED. One surface of three.** The CLI path observes and controls real Goal
and task state, and the projection the other two would consume exists and is
emitted. The TUI surface (`crates/wcore-cli/src/tui/surfaces/goals.rs`) and the
host-protocol Goal events do not exist, and no producer fixture has been
generated. The phase brief's own words apply — this criterion "cannot be closed
by tests, drive all three for real" — and one was driven.

Reported as failed rather than as "1/3 partial", because a criterion that names
three surfaces agreeing is not a third satisfied by one surface existing: the
agreement is the property, and agreement needs at least two.

## Criterion 4 — PARTIAL, upgraded from "vocabulary only"

> Session-local fixed/dynamic, event-driven, and manual loops remain bounded
> across reconnect, preemption, missed intervals, and resume.

`LoopPolicy` is no longer vocabulary only: `Fixed` is **enforced at runtime**, at
the durable boundary, and the enforcement survives a restart because the count
lives in the chain rather than in the driving process. Measured live: the
restarted process resumed at `iterations=1` and consumed 2 more, ending at 3 of
an authorized 8, with `resume_count` reaching 2 across the kill.

Still absent: `Dynamic`'s wall-clock bound, `EventDriven`'s delivery cap and
`Manual` have no runtime enforcement; no loop was preempted; "missed intervals"
is untouched. So: `Fixed` bounded across **resume** — the hardest of the four
listed conditions — and the other three conditions and three policies are not
done.

## Fenced seam request — Desktop wire contract

Per the brief, recorded rather than performed. **No lane should action this in
parallel; it conflicts deterministically.** Unchanged from the previous draft
except where marked.

```seam-request
ARTIFACT:   crates/wcore-protocol/contracts/desktop/v1/  (manifest.json + fixture trees)
GENERATOR:  crates/wcore-protocol/src/contract/generate.rs
REQUESTED:  additive minor bump adding Goal / task / wait / log / cursor fixtures
BLOCKED-BY: lane brief - "Do NOT run wcore-contract generate"

MEASURED BASE (re-read from the tree at lane/22-wire, 2026-07-27 - UNCHANGED):
  CONTRACT_MAJOR    = 1
  CONTRACT_MINOR    = 8
  GENERATOR_VERSION = wcore-desktop-contract-gen/11
  Confirmed live rather than only read: the shipped binary's `ready` frame in
  this session's json-stream transcript reports
  {"major":1,"minor":8,"generator":"wcore-desktop-contract-gen/11"}.

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

PRECONDITION - STATUS CHANGED 2026-07-27: **NOW SATISFIED.**
  The previous draft said this request was not actionable because the fixtures
  would serialize Goal state that did not exist. That is no longer true:
    - crates/wcore-agent/src/goal/ exists (kernel, ledger, fleet driver)
    - SessionEvent::GoalOpened / GoalIterationStarted / GoalWaitBegun /
      GoalWaitResolved / GoalTerminated / GoalRunResumed / GoalTaskDeclared /
      GoalTaskTransitioned are all defined and reduced
    - GoalState (with its `tasks` map), GoalTaskState, GoalTaskAttempt,
      GoalTaskCompletion and GoalTaskHandoff all serialize today, and
      `wayland-core goal status` emits exactly that projection
    - the producer is live-proven on the shipped binary across a kill/restart
  A contract generated now would describe a producer that genuinely exists.

REMAINING PRECONDITION the orchestrator must still check:
  RecoveryCursor is already the protocol crate's existing shape
  (journal_sequence + digest) and GoalState::cursor() returns it, so no second
  cursor definition needs inventing. But the Goal *command* set has only a CLI
  spelling so far - there is no typed host command enum for open/iterate/wait/
  resume/terminate. Generating command fixtures would therefore mean inventing
  the host command shape inside the generator, which is the wrong place for it.
  Sequence a typed command set first, or restrict this bump to the EVENT and
  cursor fixtures, which are fully backed today.

NOTE FOR THE ORCHESTRATOR: this lane did NOT re-stamp any contract digest and
did not run the generator. The CI contract-drift check is unaffected by this
branch.
```

## What was NOT done

* **No TUI surface.** `crates/wcore-cli/src/tui/surfaces/goals.rs` does not exist.
* **No host-protocol Goal events**, and no typed host command set — see the
  remaining precondition above.
* **No producer fixtures, no `22-04-SURFACE-PARITY.md`, no `22-04-EVIDENCE/`.**
  Parity across three surfaces cannot be captured from one.
* **No suspend / preempt / reconnect exercise.** Resume is proved; the other
  three are not.
* **`wcore-contract generate` was NOT run**, per the brief.

## Honest verdict

**Criteria 1 and 4: not met.** Criterion 1 has one of three surfaces; Criterion 4
has one of four policies enforced across one of four conditions.

What changed is that neither is blocked any more. The previous refusal was
grounded in there being no Goal for an adapter to be thin over; there is one now,
it is reachable, and its canonical projection is emitted from reduced state
rather than assembled by a surface. A TUI screen and a host adapter over that
projection are ordinary work now, not work that would have had to invent the
lifecycle first.

Reporting one surface as one surface, rather than as a third of a criterion.
