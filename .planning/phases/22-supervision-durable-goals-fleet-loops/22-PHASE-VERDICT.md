# Phase 22 — verdict against its own Success Criteria

> **SUPERSEDED TWICE. The governing grades are in `UPDATE — 2026-07-29` at the FOOT of this
> file.** Everything before it is the 2026-07-26 and 2026-07-27 gradings, retained unedited
> so the trajectory stays legible — but **do not quote them as current**. In particular the
> 2026-07-27 section's Criterion 1, 3 and 4 rows are all now known to be wrong, and its
> Criterion 4 claim that `Dynamic` and `EventDriven` have "no runtime enforcement" is
> measurably false. The phase goal is still NOT ACHIEVED; the reason has changed.

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

> **SUPERSEDED 2026-07-27 — Criterion 2 now PASSES. See the `UPDATE — 2026-07-27`
> section at the foot of this file, which carries the governing grades.** 22-03 was
> subsequently executed: the ledger drives `FleetDispatcher` through `GoalFleetDriver`,
> and the criterion was proven on Linux **and** Windows against the shipped 0.12.25
> binary — 7→0 descendants after kill, 4 drained from the outbox, effects 12/12/12, with
> the effects gate falsified to exit 1 and restored in the same run. The paragraph below
> is retained as the 2026-07-26 grading, not as the current answer.
>
> **Criterion 3 remains untouched, so the phase goal is still NOT ACHIEVED.**

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
| 2 — fleet claims survive kill/restart | FAILED, not attempted | **PASSED, both platforms, against the shipped binary** | See below. |
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

**Windows: PASSED, same numbers.** `taskkill /T /F` on the process tree at
2026-07-27T13:12:06Z: 7 descendants → 0 (2 `PING.EXE` workers, 2 `cmd.exe`, 2
`wayland-core.exe` exec-task children, 1 `conhost.exe`), killed parent confirmed
gone, restart exit 0, 2 claims revoked, 4 completions drained from the outbox,
effects 12 / 12 / 12, 14 attempts, 12 dependency releases, 0 unresolved, and the
same `produced=no reason=idempotency-key-present` on the reassigned task. Gate
falsified to exit 1 and restored in the same run.

## Claim-model conditions

* **Condition 1 — MET.** Decided 3-of-4 and recorded as an explicit amendment
  with the evidence: `22-03-CONDITION-1-DECISION.md`. `worktree_manager.rs:235`
  is a read no task owner can reach, feeding a disk-retention quota, not the
  budget accounting the condition protects.
* **Condition 2 — MET.** The Windows kill leg for the ledger ran on the previous
  lane. Note this is *not* the same as the Windows leg for the shipped binary,
  which has not run; the condition binds the former.

## What the next session should do first, in order

1. **22-02 Task 3 — the adapter surface over the five loop owners.** This is
   Criterion 3, it is the phase's hard criterion, and no lane has attempted it.
   The census in `22-02-LOOP-OWNER-CENSUS.md` already says exactly what each of
   the five produces and where Fleet must bind.
2. **The TUI Goal surface and the typed host command set**, in that order. The
   canonical projection they must consume already exists and is emitted by
   `wayland-core goal status`; the contract seam request in `22-04-SUMMARY.md`
   explains why command fixtures must wait for the typed command set.
3. **The Windows M1–M5 journal-compatibility legs** (Criterion 5), unchanged from
   the original list.

---
---

# UPDATE — 2026-07-29, lane `lane/22-remaining` — THIS SECTION SUPERSEDES BOTH ABOVE

Tree: `plan/f20-unified-audit-repair` @ `5457710e` (the 24-lane merge) plus this lane.
Graded from **source and from the shipped binary**, not from any prior summary — a summary
can itself be advertised-but-dead, which is the failure mode this phase has produced twice.

**Why the 2026-07-27 section above is stale.** It was written before lanes `22-c1`,
`22-c3` and `22-c3-goal`, all three of which are in this base. Its Criterion 1 and
Criterion 3 grades are both out of date, and its Criterion 4 grade is wrong in a way nobody
re-checked. Nothing above is edited, per this file's own convention.

## Re-graded, against the ROADMAP text verbatim

| Criterion | 2026-07-26 | 2026-07-27 | **2026-07-29** | Why |
|---|---|---|---|---|
| 1 — three surfaces observe **and control** identical state, and emit the producer fixtures | FAILED, not attempted | FAILED, 1 of 3 | **NOT MET — 3 of 3 observe, 0 of 3 control** | All three surfaces now exist (CLI verbs, `ProtocolEvent::Goal{Snapshot,Transition}`, TUI ingest + status segment). Two clauses fail: **no host→core Goal command exists** (`GoalResync` count **0** in `commands.rs`; known-positive `Stop` = 1), so "control" is delivered on ONE surface, the CLI; and the producer fixtures are **declared in `EVENT_SPECS` (8 references) but 0 of 49 fixture files on disk** are Goal fixtures. |
| 2 — fleet claims survive kill/restart/reassignment | FAILED | **PASSED** | **PASSED, unchanged** | Not re-run by this lane. Linux + Windows, shipped 0.12.25, 12/12/12 effects, gate falsified in-run. |
| 3 — five engines terminate through one canonical Goal transition, no nested owner | FAILED | FAILED, unchanged | **PARTIAL — 5/5 production paths, attachment opt-in** | The 2026-07-27 grade is stale: `22-c3` built the adapter surface and `22-c3-goal` gave all five a production path, live-proven on the shipped release. Root cause of the dead four was that `goal open` **hard-coded `GoalStrategy::Fleet`** — the product could not express four of its five strategies. Still not PASSED: attachment is **opt-in**, so an engine run with no Goal is unenforced, and **zero engine signatures changed**. |
| 4 — fixed/dynamic/event-driven/manual loops bounded across reconnect, preemption, missed intervals, resume | FAILED, vocabulary only | PARTIAL (`Fixed` only) | **PARTIAL — and the bound is wider than 2026-07-27 recorded** | That section says "`Dynamic`, `EventDriven` and `Manual` still have no runtime enforcement." **Measured, that is false for two of the three.** `GoalAuthorityRecord::iteration_ceiling` (`goal/record.rs:143`) returns a numeric ceiling for `Once`, `Fixed`, `Dynamic` **and** `EventDriven`, and `session_journal/reducer.rs:326` refuses `GoalIterationStarted` past it **at the durable boundary**. `Manual` alone has no ceiling, by design — each advance is itself an operator action. Still PARTIAL: **reconnect, preemption and missed intervals are untouched** by anyone, and only `resume` is covered (the count lives in the chain). |
| 5 — journal compatibility proved or migrated | PARTIAL | PARTIAL, unchanged | **PARTIAL, unchanged** | Not re-run by this lane. Linux-proved; the Windows M1–M5 legs were never taken. |

**Phase goal — "users can supervise durable objectives and work graphs through one
restart-safe lifecycle and one loop owner": STILL NOT ACHIEVED, and closer than either
earlier grading.**

The honest shape of the remaining gap has changed and is worth stating precisely, because
"NOT ACHIEVED" has now been recorded three times and the reason is different each time:

- 2026-07-26: nothing was reachable from any surface.
- 2026-07-27: Criterion 3 was untouched — five engines terminated five ways.
- **2026-07-29: every criterion has moved, and the goal fails on ONE word.** A user can
  open a durable Goal, drive any of five engines through it, kill the process, restart, and
  see exactly one termination — from a terminal. What they cannot do is **supervise** it
  from the TUI or a host: both surfaces are **read-only**, because a host→core Goal command
  must be answered in `crates/wcore-cli/src/main.rs`, the file every lane is fenced out of.
  "Supervise" is not "observe". One command, in one fenced file, is the difference between
  Criterion 1 NOT MET and MET — and, given C2 PASSED and C3/C4 PARTIAL, it is the single
  highest-leverage item left in this phase.

## What this lane changed, and the two F05 rows

Both capability rows the `GOAL-*` ledger row cites as its checkable blocker are closed, one
by measurement and one by construction. Full evidence in
`22-REMAINING-EVIDENCE/{midflight,learnedpolicy}/RESULT.md`.

- **`F05-TRUTH-2` (mid-flight monitor) was STALE, not unwired.** The shipped `0.12.25`
  binary's own activation stream emits
  `declared → configured → constructed → ready → reached → outcome_changed → observed`
  plus `{"type":"mid_flight_monitor_decision","directive":"replan","reason":"repeated_error"}`.
  **Both** columns of that row were false. One-variable negative control: taking the
  identical-error count from 3 to 2 takes the decision and occurrence counts from 1 to 0
  while `ready` stays 1. Both arms exit 0.
- **`F05-TRUTH-4` (learned policy) was real, and worse than recorded.** The row said the
  runtime path was unwired; in fact `AgentExecutorConfig` carried a `pub learned_policy`
  field with **zero readers in the entire workspace** while its own doc comment claimed
  `dispatch_once` consulted it. Now wired as a **narrowing-only** pre-filter (the gate is
  consulted first and its denial is final), live-proven in one run where the parent
  (`Root`) reads a file and the delegated child (`SubAgent`) gets
  `Denied by sub-agent learned policy: Read matched rule `*`` — one variable, the caller
  class. Startup truth is now `ready` from a constructed on-disk policy, else
  `disabled_by_config`; `RuntimePathUnwired` is no longer reachable for it.
  **The runtime-outcome-proof column is NOT closed** — see the limitation below.

## Deliberately left open, named rather than quietly dropped

1. **The `learned_policy` F05 outcome-proof column.** The occurrence triple is emitted on a
   real narrowing, but `OutputSink::emit_capability_activation` is a **default no-op** that
   only `ProtocolSink` overrides, and every spawned child gets `NullSink` (`Delegate`) or
   `ChannelSink` (`Spawn`/workflow), neither of which overrides it. Since `Root` bypasses
   the pre-filter by design, the occurrence can only fire inside a child, and every child
   discards it. **Generalised: no sub-agent capability activation of any kind is observable
   on any topology in this tree.** The fix needs a relay event and therefore a contract
   regeneration, which this lane may not run.
2. **Criterion 1's control half and its fixtures.** Both are fenced: the command must be
   answered in `main.rs`, and the fixtures need the single `wcore-contract generate` pass
   over the merged tree (seam request `SR-22-C1`, already fenced in `22-C1-SUMMARY.md` §6).
3. **Criterion 3's structural half.** Making an engine *incapable* of terminating outside a
   Goal means threading a token through five entry points and changing five signatures.
   That is capability breadth against a criterion already PARTIAL on a working path, and
   under Sean's 2026-07-29 scope cut it is not worth the blast radius before the deadline.
4. **Criterion 4's reconnect / preemption / missed-interval clauses**, and **Criterion 5's
   Windows M1–M5 legs** — both unchanged, both previously listed, neither attempted here.
5. **Two further stale F05 rows found incidentally**, both `CONT-*` and neither this lane's:
   the shipped binary reports `cooldown_tracker` **`ready`** (receipt says "no production
   constructor") and `pricing_refresher` `unavailable / **disabled_by_config**` (receipt
   says "no production constructor"). Reported, not edited.

---

# SUPERSEDING BLOCK — 2026-08-01, lane `verdict-truth-text`, base `02575b6f`

**This block supersedes the 2026-07-29 grade of Criterion 1 above.** It supersedes nothing else
in this file: Criteria 2, 3, 4 and 5 stand exactly as the 2026-07-29 section left them.

**Text only.** Zero files under `crates/`, `.github/`, `docs/` or `scripts/` were changed by the
lane that wrote this. No cargo was run. Full sweep, method and controls:
`.planning/VERDICT-TRUTH-2026-08-01.md`.

## Criterion 1 — **NOT MET → PARTIAL**

The 2026-07-29 row reads:

> **NOT MET — 3 of 3 observe, 0 of 3 control.** *"no host→core Goal command exists (`GoalResync`
> count **0** in `commands.rs`; known-positive `Stop` = 1) … the producer fixtures are declared in
> `EVENT_SPECS` (8 references) but **0 of 49** fixture files on disk are Goal fixtures."*

**Both of its needles are dead needles at `02575b6f`.** Re-run in this worktree, with the
known-negative control the original grading did not carry:

```
grep -c "GoalResync"        crates/wcore-protocol/src/commands.rs  ->  2   [was 0]
grep -c "GoalCancelCommand" crates/wcore-protocol/src/commands.rs  ->  2   [was absent]
grep -c "GoalZzzzz"         crates/wcore-protocol/src/commands.rs  ->  0   [KNOWN-NEGATIVE]
find crates/wcore-protocol/contracts/desktop/v1 -type f -iname "*goal*" | wc -l  ->  8   [was 0]
find crates/wcore-protocol/contracts/desktop/v1 -type f            | wc -l  -> 164   [DENOMINATOR]
```

**Control ships on all three surfaces.** `ProtocolCommand::{GoalOpen, GoalDeclareTask,
GoalAdvance, GoalCancel, GoalResync}` are declared at `crates/wcore-protocol/src/commands.rs:328`
–`:340`; `GoalCancelCommand` is the struct at `:237`, documented as terminating a Goal through
the one canonical transition with a cursor that prevents a stale host card cancelling a Goal that
has already finished.

**And a human can reach it**, which is the specific clause the row said failed:
`tui/commands/mod.rs:42` (a slash command a user types) → `TuiEngine::request_goal_control` →
`GoalControlBridge::issue_goal_control` (`tui/engine_bridge.rs:1230`, invoked at `:1273`), with a
PTY drive at `crates/wcore-cli/tests/goal_control_tui_pty.rs`. `issue_goal_control` has **10**
references across `crates/`, not the definition-plus-two-comments this row once measured.

The eight Goal artifacts are real producer output, not schemas —
`contracts/desktop/v1/commands/goal_cancel.json` is one serialized frame, byte-checked against
the live serializers by `desktop_contract_corpus.rs:202`.

## Why this is recorded as CANNOT-PASS and not merely "stale"

`GoalResync == 0` was chosen as a proxy for *"the host cannot control a Goal."* Once the command
landed, the proxy inverted — and nothing re-ran it, so this file kept publishing the zero. **A
proxy that is never re-measured is a constant, and a constant is not a gate.** That is the same
shape as the 23A-C1 defect, and the exact inverse of this phase's own 22-C3 falsifier, which
greps `orchestration/` for `GoalTerminalState` and therefore can never go green (**0** here,
against a known-positive `ClimbOutcome` = **21** in that same directory; corrected needle across
`wcore-agent/src/` = **88**). **Both directions cost the same.**

## What stays red, and is NOT swept

* **The Windows terminal leg is still NOT MEASURED**, and it needs a *different instrument*, not
  a different host — the PTY harness is `#![cfg(unix)]`. Linux and macOS are closed
  (`CRITERIA-STATUS.md`: 13 passed / 0 failed / 0 ignored / 0 filtered out on each); Windows is
  not, and this block does not claim it.
* **The *"consumed later at D2"* clause** of the criterion is untouched by this correction.
* PARTIAL, not MET, for those two reasons.

## §"Deliberately left open" item 2 is now discharged

That item reads *"Criterion 1's control half and its fixtures. Both are fenced."* Both have since
landed: the command exists in the protocol and the fixtures are on disk from a contract
regeneration. **Retired, not deleted** — the record of it having been correctly fenced at the
time is the point.

_Corrected 2026-08-01 · base `02575b6f` · lane `verdict-truth-text` · source measurement only,
two-directional controls, no cargo, no `crates/` edit._
