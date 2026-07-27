---
phase: 22-supervision-durable-goals-fleet-loops
plan: "03"
subsystem: orchestration
tags: [fleet, ledger, wire, cli, kill-restart, shipped-binary, idempotency]
requires:
  - "22-01"
  - "22-02"
  - "22-03 Tasks 1-4"
provides:
  - the durable task ledger wired into FleetDispatcher as its source of work
  - a user-reachable Goal surface, so the kill/restart proof runs on the shipped binary
  - Condition 1 of the claim model, decided and closed with cross-audited evidence
affects:
  - crates/wcore-agent
  - crates/wcore-cli
tech-stack:
  added: []
  patterns:
    - "one claimed task becomes one MeshAgent whose closure owns its TaskAuthority"
    - "record the completion inside the agent; drain the outbox from the ledger, never from a dispatcher return value"
    - "the idempotency marker IS the effect, created after the worker succeeds"
key-files:
  created:
    - crates/wcore-agent/src/goal/fleet.rs
    - crates/wcore-agent/tests/goal_fleet_wire_test.rs
    - crates/wcore-cli/src/goal_cmd.rs
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-03-CONDITION-1-DECISION.md
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-03-EVIDENCE/wire-live/
  modified:
    - crates/wcore-agent/src/goal/mod.rs
    - crates/wcore-cli/src/lib.rs
    - crates/wcore-cli/src/main.rs
decisions:
  - "Condition 1 is FULLY MET: worktree_manager.rs:235 leaves the authoritative surface, cross-audited 3-of-4 and recorded as an explicit amendment"
  - "The parent owns every ledger write, because the swarm worker sandbox structurally forbids a worker writing the shared journal"
  - "The wire targets FleetDispatcher, not Swarm's worktree fanout; that limit is stated rather than blurred"
metrics:
  duration: one session
  completed: 2026-07-27
status: complete
---

# Phase 22 Plan 03, the wire: the ledger becomes the Fleet dispatcher's work

**Success Criterion 2: I grade it PASSED on Linux, against the shipped
`wayland-core` binary, and NOT YET on Windows.** Both of the two named grounds on
which three previous agents graded it FAILED are closed on Linux. The Windows leg
is a build that did not finish inside this session, not a defect — and until it
runs, "on both platforms" is not claimed. Full verdict at the end.

## The two grounds, and what closed them

### Ground 1 — the ledger was not wired into `FleetDispatcher`/spawner

**Closed.** `crates/wcore-agent/src/goal/fleet.rs` — `GoalFleetDriver` — makes
the ledger the dispatcher's source of work. One claimed task becomes exactly one
`MeshAgent` whose closure **owns** its `TaskAuthority`, so an agent cannot record
an effect for a task it did not win: it does not hold the only type that admits
the call.

Three design points that are load-bearing rather than stylistic:

**The parent owns every ledger write, and this is structural.** A swarm worker
runs under a manifest whose `fs_write_allow` is exactly its own checkout and
scratch, with `network = Deny` (`wcore-swarm/src/dispatch.rs::worker_manifest`).
A worker process therefore *cannot* write the shared journal. A pull model in
which each worker claims its own task is not expressible through that boundary —
and should not be, since it would mean handing a worker authority over the
parent's chain. So the parent claims, and each `TaskAuthority` lives and dies
inside the parent process. It is never serialized, never sent to a child, never
reconstructed from one, which is what makes "no `Deserialize`" a property rather
than a decoration.

**The completion is recorded inside the agent; the outbox is drained from the
ledger.** Both dispatchers under this driver can lose an `AgentReport`, and
neither loss is a bug in them: `MeshDispatcher::dispatch` wraps its `JoinSet` in
a `tokio::time::timeout` and on expiry drops the set — aborting every in-flight
agent *and* discarding every report already collected — while
`FleetDispatcher::dispatch` returns on the first shard error, dropping its own
`JoinSet` and aborting the surviving shards. A driver that learned its outcomes
from the return value would lose completions whose work genuinely happened. So
the agent commits durably *before* returning its report, and the parent then
drains `pending_deliveries` — a replay of the chain. The report is pure telemetry
and dropping it costs nothing.

**Every wave consumes one Goal iteration through `GoalKernel::start_iteration`,**
so the loop bound is enforced by the reducer at the durable boundary rather than
by the driver counting — a count the kill would erase.

### Ground 2 — no proof against the shipped `wayland-core` binary

**Closed on Linux.** `crates/wcore-cli/src/goal_cmd.rs` adds `wayland-core goal`
with six verbs: `open`, `task`, `run`, `status`, `exec-task`, `effects`. The
live proof below drives **only** these verbs against a release binary. The
`examples/` instrument is no longer the strongest available evidence and is not
what this result rests on.

`exec-task` is a product verb, not a fixture. The exactly-once property needs a
second half at the effect boundary — an atomic `create_new` keyed by the task's
idempotency key — and that half must run in the process that produces the effect,
not in a parent that may die between checking and spawning. Putting it in the
shipped binary makes "no duplicate effect after a kill" a property of the product
rather than of whichever harness measured it.

## Live evidence — shipped binary, real SIGKILL, real restart

`wayland-core 0.12.25`, release build, `hetzner-dsm`. Capture:
`22-03-EVIDENCE/wire-live/linux/live-capture.txt`; script:
`22-03-EVIDENCE/wire-live/live-linux-proof.sh`.

**The scenario carries state, deliberately.** A fleet dispatcher with one worker,
an empty queue and a zero-length history is a scenario in which broken code looks
correct, so: 12 tasks (not 1); width 6 at shard size 2, so each wave genuinely
shards into 3; a dependency layer, so more than one wave is required; the kill
lands **mid-wave** with some tasks recorded-but-undelivered and others still
running, not at a quiescent boundary; and one task's effect is placed on disk
with **no** completion before the restart, which is the only state in which the
idempotency key can be observed doing anything at all.

| | Measured |
|---|---|
| Kill | `kill -9 -<PGID>` on the process group at **2026-07-27T12:13:53Z** |
| Process-group members before → after | **7 → 0** |
| Live 40s worker children before → after | **2 → 0** |
| Effects on disk at the kill | 4 (t00–t03 recorded, **none delivered**) |
| Restart exit code | **0** |
| Claims revoked on lease expiry | **2** (t04, t05) |
| Completions drained from the outbox | **4** — the ones the dead parent never observed |
| Effects: total / distinct / expected | **12 / 12 / 12** |
| Attempts | **14** = 12 tasks + 2 reassignments |
| Dependency releases | **12**, one per task |
| Unresolved | **0** |
| Shards, wave 0 / wave 1 | **3 / 1** |

The interesting task is **t04**. Its effect was on disk with no completion — the
exact state a kill leaves between a worker's write and its parent's record. On
restart the ledger revoked the orphaned claim, reassigned it at epoch 2, and the
re-run found the key and did not write again:

```
GOAL-EXEC: task=t04 key=idem-t04 produced=no reason=idempotency-key-present
```

and it still ends `attempts=2 epoch=2 completed=True delivered=True`. Duplicate
execution of the process happened; duplicate **effect** did not, which is what
the criterion forbids.

**The effects gate can go red.** `goal effects --expect 12` is a real gate, not a
print: a duplicated effect file took it to `total=13 distinct=12` and **exit 1**,
and removing the duplicate returned it to exit 0. Verified in the same run.

## Falsification — every load-bearing guard was made to fail

Neutralized one at a time against all three goal suites
(`falsify-out/` on the build host):

| Guard neutralized | Result |
|---|---|
| Epoch fence (`require_live_epoch` equality) | **1 red**, precisely the fence test |
| Live-claim exclusion in `claimable_tasks` | **1 red**, the shard-abort test |
| Idempotency key made per-attempt | **2 red** |
| Agent does not record durably | **6 red** |
| `run_wave` propagates the transport failure | **1 red**, the shard-abort test |
| Loop bound at the durable boundary | **1 red** |
| Dependency gate at the durable boundary | **1 red** — in `goal_fleet_ledger_test`, not mine (see below) |

The last one is worth stating precisely, because it is the "already green" class.
Neutralizing the durable-boundary dependency gate left my **entire wire suite
green**: my driver never claims past its own `claimable()` query, so my tests
reach the query gate and not the durable one. The gate is nonetheless pinned —
by the previous lane's `a_task_with_unmet_dependencies_is_not_claimable_and_
unblocks_exactly_once`, which claims past the query deliberately. So the gate can
go red; my suite is simply not what makes it. Measured rather than assumed, and
reported rather than left as an unexamined green.

## Findings

| ID | Severity | Status | Summary |
|---|---|---|---|
| F-10 | **HIGH** | **FIXED (mine)** | `exec-task` created the idempotency marker **before** running the operator's command. A worker killed or failing mid-run left the marker with no effect, and every later retry then found it and declined — the task became permanently un-runnable and its effect never happened. That is a lost completion wearing an exactly-once costume, and it fails the criterion exactly as loudly as a duplicate. Marker creation moved after the worker succeeds; the marker IS the effect now, one `create_new` + payload + fsync. Guard: `a_failed_worker_leaves_the_task_runnable_rather_than_permanently_blocked`, which fails against the old ordering. |
| F-11 | MEDIUM | **FIXED (mine)** | The driver stamped its own supervisor identity as the attempt child's parent session, and the reducer refused every claim (`parent session does not match journal authority`) — correctly, since a child claiming a foreign parent is a lineage forgery. Found by the wire tests on their first run, not by review. Field renamed `supervisor_id` so the two identities cannot be confused again. |
| F-12 | MEDIUM | **PRE-EXISTING, not mine** | The journal's writer lease refuses a second opener, and it is `#[cfg(unix)]`-gated. On Windows two supervisor processes can hold one journal and only the epoch fence stands between them. Already recorded as threat T-22-06 in the phase verdict; now pinned by `a_second_opener_is_refused_the_writer_lease_on_unix`, which should be un-gated if that ever closes. BACKLOG. |
| F-13 | — | **DISPROVED, not filed** | All four panel members predicted cross-process over-admission via the stale retained-worktree count. Measured against the shipped binary: `MAX_RETAINED_WORKTREES` is **256** — never binding at any width the CLI permits — and the gate that actually binds is `reserved_workspace_bytes`, re-read from disk at *each* workspace creation. Two concurrent 6-worker dispatches on one repo: peak 8 roots of 12 requested, excess refused individually, one process exited 1. Reported as disproved rather than filed. |
| F-8 | MEDIUM | **PRE-EXISTING** | Windows `wcore-cron` clippy exits 101. Not chased, not mine. |
| F-9 | MEDIUM | **PRE-EXISTING** | Linux `workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` times out. Reproduced at HEAD; not chased, not mine. |
| F-14 | MEDIUM | **PRE-EXISTING, proven** | `wcore-cli::child_authority_corpus` fails 4 tests (`corpus_time`, `corpus_cost`, `corpus_token`, `corpus_depth`). Single-variable proof: identical `23 passed / 4 failed` at HEAD `08e6869a` **and** at base `6c49d953`, same warm target, same command. The failure text names its own cause — another lane added a child budget request channel to `spawner.rs`, which this lane never touched — and says "This is EXPECTED from 2026-07-27". BACKLOG, not mine. |

## Gate results, honestly

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | **PASS** — and **green at base**, unlike the previous lane |
| `cargo clippy -p wcore-agent -p wcore-types -p wcore-swarm -p wcore-cli --all-targets --all-features -D warnings` (Linux) | **PASS**, exit 0 |
| `goal_fleet_wire_test` (Linux) | **PASS 10/10** |
| `goal_fleet_ledger_test` / `goal_kernel_test` (Linux) | **PASS 11/11 / 10/10** |
| `wcore-cli --lib goal_cmd` (Linux) | **PASS 7/7** |
| `cargo nextest run -p wcore-agent -p wcore-types -p wcore-swarm -p wcore-cli` | **5364 passed, 4 failed, 1 timed out** — all five proven pre-existing at base (F-8/F-9/F-14) |
| Live kill/restart on the **shipped binary** (Linux) | **PASS** — 12/12/12, restart exit 0 |
| The live effects gate, falsified | **PASS** — goes red on a duplicate, green when removed |
| Live kill/restart on the shipped binary (**Windows**) | **NOT RUN** — release build did not finish in session |
| Bare full-workspace aggregate | **NOT RUN** — the lane brief forbids it; scoped to four crates instead |

## Condition 1 — DECIDED and CLOSED

Full record with the panel's raw output:
`22-03-CONDITION-1-DECISION.md`.

**Option A, 3 of 4** (Gemini A, Kimi A, internal adversarial pass A; Codex B).
`worktree_manager.rs:235` is `retained_worker_count` — a read that writes
nothing, whose only production caller is a process-wide admission gate evaluated
once before any worker exists, and which **no task owner can reach**, since swarm
workers are separate processes whose sandbox permits writes only to their own
checkout and scratch. The accounting Condition 1 protects is the *budget
reservation* accounting, which **is** fenced in the reducer; what this feeds is a
disk-retention evidence quota and an output-byte budget.

Codex's dissent is recorded in full. Its substance concedes the mechanism — "this
side-effect-free, pre-owner read cannot commit a stale task effect, and adding a
task-epoch check would prevent no demonstrated failure" — and its real objection
is procedural: that the condition must be **amended in writing** rather than
declared met by re-derivation. That objection is correct and the decision record
is the amendment it asks for.

The dependency constraint (`TaskAuthority` must be unforgeable, so it lives in
`wcore-agent`, which sits above `wcore-swarm`) is true and is **not** the reason
this is closed. Had the path been authoritative, that constraint would have been
a reason to do the work, not to grant an exemption.

## What was NOT done, plainly

* **The Windows live leg did not run.** The release build was started early and
  was still compiling when the session ended. Everything else is in place — the
  worktree is at the lane commit and the scenario script is platform-agnostic —
  but a leg that did not run is recorded as not run, never inferred from Linux.
  Until it does, Criterion 2 is closed on **one** platform.
* **The wire targets `FleetDispatcher`, not `Swarm`'s worktree fanout.**
  `FleetDispatcher` is the type named in this plan's `files_modified` and it is
  the one that structurally supports per-task binding, since its agents are
  distinct closures. But the worktree `Swarm` path — the one with checkouts,
  sandboxes and `flock` leases — is **not** driven by the ledger. Saying "wired
  into the Fleet dispatcher" must not be read as "the worktree fanout is
  ledger-driven"; it is not.
* **`spawner.rs` is untouched**, despite being in `files_modified`. The driver
  spawns task processes through the CLI's `exec-task`, not through the durable
  child spawner. Wiring the ledger to `DurableChildStore`'s lifecycle is a real
  and separable piece of work; it is named rather than half-done.
* **No TUI surface and no host-protocol surface.** Criterion 1 needs all three;
  this delivers one. See `22-04-SUMMARY.md`.
* **The residual window in `exec-task` is stated, not closed.** A kill between
  `create_new` and the payload write leaves a present-but-empty effect file. It
  still counts as produced and is still counted exactly once, so nothing the
  criterion measures changes; closing it entirely needs an atomic
  write-then-link, which buys nothing here.

## Deviations

* **[Rule 1 — bug] Fixed F-10 in my own new code**, found while designing the
  live scenario rather than by a test — the test came second, as a guard.
* **[Rule 1 — bug] Fixed F-11**, found by the wire tests on their first run.
* **[Rule 3 — blocking] Refactored `exec_task` to take its assignment as
  parameters** rather than reading process environment, after clippy flagged a
  `MutexGuard` held across an await. The lock existed only because the tests had
  to mutate global state to reach the function; removing the coupling removed
  both the lint and a real flakiness source that would have looked exactly like
  the idempotency gate failing.
* **[scope] Did not touch `spawner.rs`**, listed in `files_modified`. Reason above.

## Honest verdict

**Success Criterion 2: PASSED on Linux, against the shipped binary. NOT CLOSED
on Windows.**

What is true, and is new: the ledger is now the Fleet dispatcher's source of
work through a user-reachable path, and claims, dependencies, completions and
reassignment survive a real uncatchable kill of a real process tree and a real
restart **of `wayland-core` itself** — proved by counting effects a dead process
left on disk, with a gate that was made to go red. The idempotency key was
observed refusing a duplicate in the exact reassignment path a kill produces.
Three agents carried "the proof is a harness, not the product" forward honestly;
that caveat is now retired on Linux and I am saying so explicitly, because
blurring it would waste the honesty that got us here.

What is not true: the criterion has been graded on both platforms throughout this
phase, and I have one. The Windows leg is a compile that ran out of session, and
a leg that did not run cannot be inferred from the leg that did — which is the
exact error that produced F-1 earlier in this phase, where a race Windows
happened to win was read as a race that was absent.

So: the mechanism is closed, the wire is closed, the shipped-binary proof is
closed on Linux, and the platform coverage is not. Reporting that split is worth
more than claiming the whole.
