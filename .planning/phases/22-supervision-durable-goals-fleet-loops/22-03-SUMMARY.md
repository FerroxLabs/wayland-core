---
phase: 22-supervision-durable-goals-fleet-loops
plan: "03"
subsystem: orchestration
tags: [fleet, durability, kill-restart, baseline, swarm]
requires:
  - "22-01"
  - "22-02"
provides:
  - the measured pre-ledger durability baseline of the real fanout on Linux
  - three product defects in the Fleet path, one fixed with a falsifiable guard
  - the falsification of the "AppContainer is unavailable over SSH" environment lore
affects:
  - crates/wcore-swarm
tech-stack:
  added: []
  patterns: []
key-files:
  created:
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-03-CLAIM-MODEL.md
    - .planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md
    - .planning/intel/appcontainer-lease-wedge-probe.ps1
  modified:
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_manager.rs
    - crates/wcore-swarm/src/worktree_tests.rs
    - crates/wcore-swarm/src/worktree_tests/linux.rs
    - Cargo.lock
decisions:
  - "No claim-revocation model was chosen: the panel was not convened, so no option is recorded rather than one being estimated"
  - "Stopped before building the ledger rather than building it on a kernel that does not exist"
metrics:
  duration: one session
  completed: 2026-07-27
status: partial
---

# Phase 22 Plan 03: Durable Fleet Task Ledger — Summary

**Termination state: none of the plan's four.** The honest label is **Task 1
delivered, Tasks 2–4 not run, with three product defects found in the course of
Task 1 and one of them fixed.**

**F22-03 is NOT complete. Success Criterion 2 is NOT closed.**

## What the plan assumed, and what was actually there

The plan opens: *"Fleet today has a dispatcher and a board, not a ledger, and the
difference is the whole requirement."* Task 1 was supposed to measure what that
fanout loses on a kill, so Tasks 2–4 could build a ledger above it.

The measurement found that the fanout **did not fan out**. At `--workers 8` the
shipping `swarm` subcommand ran **one** worker and refused the other seven, while
exiting **0**; and a second dispatch against the same repository failed outright
with zero workers. Both because the swarm mints `.swarm-worktrees/` inside the
repository whose cleanliness its own dirty-checkout guard judges.

That had to be fixed before "duplicate execution across a fleet" was even a
measurable idea, and fixing it plus establishing the real baseline consumed the
task. Full evidence is in `22-03-CLAIM-MODEL.md`.

## Findings

| ID | Severity | Status | Summary |
|---|---|---|---|
| F-1 | HIGH | **FIXED** (`f43c279c`, guard `1fab91f7`) | Fanout refused dispatch over its own worktree root: effective parallelism 1 at width 8, exit 0; second dispatch refused entirely. |
| F-2 | HIGH | **NOT FIXED** | A killed fanout cannot be restarted: orphaned reservations exhaust the aggregate workspace budget and nothing reclaims them at dispatch. |
| F-3 | HIGH | **NOT FIXED** | Workers fail as a function of elapsed run time — 4/4 at 1s, 1/4 at 10s, `invalid retained workspace reservation`. Reproduced on Windows. |
| F-4 | HIGH | reported, not fixed (other crate) | `wcore-sandbox` acceptance tests write leases into the **production** lease directory, permanently disabling the Windows sandbox. See the intel note. |

F-1's fix is proved red-before/green-after on the real binary: 1/8 → 8/8, and
restart unblocked at that layer. The regression guard was **verified to fail**
against a neutralized fix — the first draft was self-passing and was corrected.

## Gate results, honestly

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | **PASS** |
| `cargo build --release --locked -p wcore-cli` (Linux) | **PASS** at `ba4e541e` — and **RED at base**, see below |
| `wcore-swarm` `assert_clean` tests | **PASS** 2/2 |
| Falsifiability of the new guard | **PASS** — fails with the fix neutralized, passes with it |
| `worktree_add_timeout_kills_tree` / `cancelled_cleanup_kills_git` | **PASS** 2/2 with blocking fixtures |
| Full Linux aggregate (`nextest --profile ci`) | **NOT RUN** |
| Windows clippy + ledger suite | **NOT RUN** (no ledger exists) |
| Live kill/restart test (Task 4) | **NOT RUN** |

`cargo build --locked` was **red at base for every lane**: `Cargo.lock` omitted
`wcore-exec-backend` and a `chrono` dependency. Regenerated in `ba4e541e`,
24 insertions / 0 deletions, no version changed.

## Live evidence

Real release binary, real fanout, real uncatchable kill on Linux at
`2026-07-27T01:21:45Z` (`kill -9 -<PGID>`): 8 workers mid-flight, 8 `START`
lines, 0 `DONE`, 0 heartbeat files. All 8 `bwrap` containers and 17 worker shells
reaped to 0 — **no orphan completed untracked work**. Restart detected the crash
sentinel and then refused: *"dispatch aggregate workspace budget is already
committed"*.

## What was NOT done, plainly

- **Task 2 — the four-way panel: not convened.** No claim-revocation model was
  chosen, and no `OPTION:` lines with duplicate-execution / lost-completion
  windows exist. They are absent rather than estimated, because numbers invented
  without the measurements behind them are the specific forgery this plan names.
- **Task 3 — the ledger: not built.** `crates/wcore-agent/src/goal/` does not
  exist. 22-01 shipped the vocabulary in `wcore-types::goal` but never built the
  kernel, the `SessionEvent` variants, the reducer arm or the
  `ReducedSessionState` field, so there was nothing for a ledger to extend. This
  is a missing dependency, not a finding against 22-01's record shape.
- **Task 4 — the live kill/restart of the ledger: not run**, since there is no
  ledger. The *pre-ledger* baseline kill was run on Linux.
- **Windows kill leg: not validly run.** Failed on my own harness (PowerShell
  `ArgumentList` split `cmd.exe /c worker.cmd`), recorded as unmeasured rather
  than inferred from Linux.
- No panel, no fencing mechanism, no idempotency key, no completion outbox, no
  reassignment path, no `goal_fleet_ledger_test.rs`, no
  `goal_fleet_restart_live_test.rs`.

## Deviations

- **[Rule 3 — blocking] Regenerated `Cargo.lock`.** `--locked` failed at base;
  nothing could be built or measured until it was fixed. Cross-lane item.
- **[Rule 1 — bug] Fixed F-1 in `wcore-swarm`**, outside this plan's
  `files_modified`. Without it the plan's own success criterion was not
  measurable. Recorded rather than hidden.
- **[out of plan] Wrote `.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`** at the
  coordinator's request.
- **[out of plan] Changed two `wcore-swarm` test fixtures from busy-spin to
  blocking** at the coordinator's request; committed separately (`f8260568`).

## Honest verdict

The plan's criteria were **not met**. What was delivered instead is the thing
that had to be true first: the Fleet fanout now actually fans out, the real
durability baseline is measured on Linux rather than assumed, and two further
HIGH defects that would have invalidated any ledger built on top are named with
evidence.

A ledger built this session would have sat on a kernel that does not exist, above
an executor that could not run more than one worker and could not restart after a
kill. Reporting that is worth more than shipping it.
