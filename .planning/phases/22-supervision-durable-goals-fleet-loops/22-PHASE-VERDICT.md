# Phase 22 — verdict against its own Success Criteria

Tree: `2ecdfdf54ff7fda920eec7d068337006e5da4ee4` + this phase's commits.
Graded 2026-07-26 by the executing agent. Criteria quoted verbatim from
`.planning/ROADMAP.md`.

**Goal (verbatim): "Users can supervise durable objectives and work graphs through
one restart-safe lifecycle and one loop owner."**

**NOT ACHIEVED.** A user cannot supervise anything this phase built, because
nothing this phase built is reachable from the CLI, the TUI or the host protocol.

---

## Criterion 1

> CLI, TUI, and host-protocol paths observe and control identical Goal, child,
> task, wait, log, cursor, and terminal producer state, and emit the canonical
> serialized producer fixtures consumed later at D2.

**FAILED — not attempted.** Plan 22-04 was not executed. There is no Goal
command, no Goal event, no TUI surface and no producer fixture. Zero of the three
surfaces were driven. The phase brief said this criterion "cannot be closed by
tests — drive all three for real", and none of the three was driven for Goal
state at all.

## Criterion 2

> Fleet claims and dependencies survive kill/restart/reassignment without
> duplicate execution or lost completion.

**FAILED — not attempted.** Plan 22-03 was not executed. No task ledger, no
claim, no heartbeat, no revocation model. No process was killed mid-fanout on
either platform.

## Criterion 3

> Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one canonical
> Goal transition with no nested verification/retry owner.

**FAILED — measured, not built.** This was the phase's hard criterion and the
work stopped one step short of it.

What exists: the five owners were measured against the source
(`22-02-LOOP-OWNER-CENSUS.md`), which produced a design correction the plan itself
had missed — Anvil already carried a ten-variant terminal enum with exactly the
required discipline, so the canonical taxonomy is a LIFT of it rather than a
sixth vocabulary. That taxonomy shipped and is green
(`wcore_types::goal::GoalTerminalState`, 7 tests, Linux). The census also
established that only Anvil can reach a verified state, because every other
engine's verification owner is a model judge, a shape validator or a boolean
count — and that fact is now written down in exactly one place.

What does not exist: the adapter surface. After this phase the five engines still
return `ClimbOutcome`, `CouncilRunResult`, `WorkflowRunError`, a caller-chosen
`T`, and nothing respectively. A taxonomy everything *could* map onto is not a
construction where nothing can terminate any other way, and the plan was explicit
that a convention is not the property.

## Criterion 4

> Session-local fixed/dynamic, event-driven, and manual loops remain bounded
> across reconnect, preemption, missed intervals, and resume; persistent
> scheduling is deferred explicitly to Phase 24.

**FAILED — vocabulary only.** `LoopPolicy` exists and its `Dynamic` variant is
structurally incapable of expressing a single-bound loop, which is a real if
small property. Nothing enforces any bound at runtime; no loop was suspended,
resumed, preempted or reconnected.

## Criterion 5

> Existing journal compatibility is proved or migrated explicitly without
> silently invalidating F12 behavior.

**PARTIAL — the strongest result in the phase, and still short of the clause.**

Proved, on Linux, cross-binary and single-variable, with a real 84,327-byte
journal written by the shipped binary and read by a differently-built one:
reduction byte-identical by SHA-256 (M1); an appended record moves nothing but
the chain head (M2); an old binary on a new journal fails CLOSED with zero bytes
of reduced state rather than truncating silently (M3); a pre-change snapshot and
authority binding are accepted unchanged (M4); the writer lease is released on
drop (M5). Authorized 4-of-4 by an external panel with the verdict itself put to
audit and upheld with zero unsound votes.

Two clauses are unmet:

- **"proved" is Linux-only.** The Windows binary built and produced a real
  81,093-byte WJ01 journal, but the reduce instrument needed for the cross-binary
  comparison died mid-build on a contended box (`EXIT=-1`) and the Windows halves
  of M1–M5 were never taken. The writer lease is `#[cfg(unix)]`-gated and Windows
  byte-range locks are mandatory rather than advisory — a recorded prior defect
  class (threat T-22-06) that this phase did not close.
- **"without silently invalidating F12 behavior" has no guard.** The corpus is
  retained as evidence but no test pins its reduction. The moment someone changes
  reduction semantics, nothing goes red. The determination is a snapshot, and a
  snapshot is not a canary.

Also: neither corpus contains a `tool_execution_*` frame, because the provisioned
Anthropic credential returns HTTP 401 on both hosts and supplying a working one is
reserved to Sean. The tool region is the densest part of the reduced state and
this determination does not touch it.

---

## Summary

| Criterion | Grade |
|---|---|
| 1 — three surfaces observe identical state | FAILED, not attempted |
| 2 — fleet claims survive kill/restart | FAILED, not attempted |
| 3 — one canonical terminal transition | FAILED, measured not built |
| 4 — bounded session-local loops | FAILED, vocabulary only |
| 5 — journal compatibility proved or migrated | PARTIAL, Linux-proved, Windows open, no regression guard |

**Phase goal: NOT ACHIEVED.** One of five criteria is partial; four are failed.

## What the next session should do first, in order

1. **Take the Windows M1–M5 legs.** `C:\p22` is already a detached worktree at
   the right commit and `wayland-core.exe` is already built; only the reduce
   instrument is missing, and the example source is already in place at
   `C:\p22\crates\wcore-agent\examples\p22_reduce.rs`. Build it when the box is
   quiet. This closes the largest hole in the only criterion that got anywhere.
2. **Build the kernel and the stored-corpus canary** (22-01 Task 3). The record
   shape is authorized and the taxonomy is landed; the remaining work is the
   `SessionEvent` variants, the reducer arm, the `ReducedSessionState` field
   (which MUST be `#[serde(default, skip_serializing_if = ...)]` or M1's
   byte-identity property stops holding), and a test that pins
   `22-01-EVIDENCE/linux/session-journal.bin`'s reduction forever.
3. **Then 22-02 Task 3** — the adapter surface. The census already says exactly
   what each of the five produces and where Fleet must bind.

## A note on why this phase got as far as it did and no further

The plan set is four plans, fourteen tasks, and nearly every task carries a live
Linux leg AND a live Windows leg. Two cold release builds of `wcore-agent` on a
Windows box shared with six concurrent phases consumed the majority of the
session's wall time, and the second one died. That is a real capacity fact about
this program's environment, not a scheduling accident: a phase whose every task
requires a serialized turn on one contended physical machine cannot be executed
in one session, and planning that assumes otherwise will keep producing this
outcome.

---
---

# UPDATE — 2026-07-27, lane `lane/22-wire`

The grades above were written on 2026-07-26 against a tree in which
`crates/wcore-agent/src/goal/` did not exist. Three lanes have since run. This
section re-grades every criterion against the current tree and supersedes the
summary table above; nothing above it is edited, so the trajectory stays legible.

## Re-graded

| Criterion | Was | Now | Why |
|---|---|---|---|
| 1 — three surfaces observe identical state | FAILED, not attempted | **FAILED**, one surface of three | `wayland-core goal` exists and emits the canonical projection; no TUI surface, no host-protocol Goal events, no fixtures. Agreement needs at least two. |
| 2 — fleet claims survive kill/restart | FAILED, not attempted | **PASSED on Linux against the shipped binary; NOT CLOSED on Windows** | See below. |
| 3 — one canonical terminal transition | FAILED, measured not built | **FAILED**, unchanged | The five engines still return `ClimbOutcome`, `CouncilRunResult`, `WorkflowRunError`, a caller-chosen `T`, and nothing. The ledger uses `GoalTerminalState` for task outcomes, which is one more real consumer of the taxonomy, but the adapter surface over the five owners was not built. No lane attempted 22-02 Task 3. |
| 4 — bounded session-local loops | FAILED, vocabulary only | **PARTIAL** | `LoopPolicy::Fixed` is now enforced by the reducer at the durable boundary and the bound survives a restart, because the count lives in the chain. `Dynamic`, `EventDriven` and `Manual` still have no runtime enforcement, and preemption / missed intervals are untouched. |
| 5 — journal compatibility proved or migrated | PARTIAL | **PARTIAL**, unchanged | The Windows M1–M5 legs were still not taken. The `tasks` field added by the ledger carries `skip_serializing_if`, and F-7 added the falsifiable guard the original grading said was missing, so the "no regression canary" half is better than it was. The Linux-only half is not. |

**Phase goal — "users can supervise durable objectives and work graphs through one
restart-safe lifecycle and one loop owner": STILL NOT ACHIEVED.**

A user can now open a durable Goal, declare a task graph, run it, kill it, and
restart it from a terminal, and the work survives — that is real and it is new,
and it is why Criterion 2 moved. But "one loop owner" is Criterion 3, and
Criterion 3 did not move: five engines still terminate five ways. A phase whose
hard criterion is untouched has not achieved its goal, however much else landed.

## Criterion 2, in detail

> Fleet claims and dependencies survive kill/restart/reassignment without
> duplicate execution or lost completion.

**PASSED on Linux, against `wayland-core 0.12.25` release, driving only shipped
verbs.** `kill -9` on the process group at 2026-07-27T12:13:53Z, mid-wave:
7 process-group members → 0, 2 live worker children → 0, restart exit 0,
2 claims revoked on lease expiry, 4 completions drained from the outbox that the
dead parent never observed, effects **12 total / 12 distinct / 12 expected**,
14 attempts (12 + 2 reassignments), 12 dependency releases, 0 unresolved. The
counting gate was falsified in the same run: a duplicated effect took it to 13
and exit 1.

This retires the caveat three successive summaries carried honestly — that the
proof was a real process but a test harness, not the product. It is now the
product. Full record: `22-03-SUMMARY-WIRE.md`.

**NOT CLOSED on Windows.** The release build did not finish inside the session.
A leg that did not run is recorded as not run and is never inferred from Linux —
which is the exact error F-1 punished earlier in this phase, where a race Windows
happened to win was read as a race that was absent. Everything needed is staged:
`C:\p22gk` is at the lane commit and
`22-03-EVIDENCE/wire-live/live-windows-proof.ps1` is the platform-matched
scenario, using `taskkill /T /F` for the uncatchable tree kill.

## Claim-model conditions

* **Condition 1 — MET.** Decided 3-of-4 and recorded as an explicit amendment
  with the evidence: `22-03-CONDITION-1-DECISION.md`. `worktree_manager.rs:235`
  is a read no task owner can reach, feeding a disk-retention quota, not the
  budget accounting the condition protects.
* **Condition 2 — MET.** The Windows kill leg for the ledger ran on the previous
  lane. Note this is *not* the same as the Windows leg for the shipped binary,
  which has not run; the condition binds the former.

## What the next session should do first, in order

1. **Take the Windows shipped-binary leg.** One command once the build lands:
   `powershell -File 22-03-EVIDENCE/wire-live/live-windows-proof.ps1`. This is the
   only thing standing between Criterion 2 and a two-platform pass.
2. **22-02 Task 3 — the adapter surface over the five loop owners.** This is
   Criterion 3, it is the phase's hard criterion, and no lane has attempted it.
   The census in `22-02-LOOP-OWNER-CENSUS.md` already says exactly what each of
   the five produces and where Fleet must bind.
3. **The TUI Goal surface and the typed host command set**, in that order. The
   canonical projection they must consume already exists and is emitted by
   `wayland-core goal status`; the contract seam request in `22-04-SUMMARY.md`
   explains why command fixtures must wait for the typed command set.
4. **The Windows M1–M5 journal-compatibility legs** (Criterion 5), unchanged from
   the original list.
