# FINDING — CI has produced no verdict on the integration branch since the clippy unblock

**Severity: HIGH.** Measured 2026-07-29 from the main thread, integration `15cda12d`.

**This corrects `HANDOFF-2026-07-29.md` §1 and `MILESTONE-RC.md` §2 row 2.** Both say CI was
"restored" on 2026-07-29. The clippy blocker *was* removed and the test step *did* execute once.
But CI on `plan/f20-unified-audit-repair` has **not started a single job since**, and the
"12,838 tests, 68 failures → 2" figure is **lane-local `nextest` on hetzner, not CI**. That is
legitimate evidence and it is not the same claim.

---

## 1. What was measured

`ci-unblock` merged at **2026-07-28T20:43Z** (`3687cbc2`). Every `CI` workflow run on the
integration branch created after that timestamp is `cancelled`, and **every one has
`jobs.total_count == 0`** — they never got a runner:

| run | created | cancelled | jobs |
|---|---|---|---|
| 30423289490 | 04:49:00Z | 05:19:00Z | **0** |
| 30423121521 | 04:45:09Z | 04:49:01Z | **0** |
| 30421306254 | 04:05:12Z | 04:05:46Z | **0** |
| 30420905205 | 03:56:29Z | 04:05:13Z | **0** |

Each cancellation lands within one second of the *next* push's run being created. Over the last
100 runs on this branch: **83 cancelled, 11 failure, 4 success — and all 4 successes are the
`macOS native suites (Phase 28)` workflow, not `CI`.** The `CI` workflow has **zero** successes
on this branch in that window (2026-07-28T01:48Z → 2026-07-29T05:18Z).

## 2. Root cause — and it is NOT the concurrency guard

The obvious suspect is wrong, and worth recording because it would have produced a plausible
fix that changed nothing. `ci.yml:47-53` already sets
`cancel-in-progress: ${{ github.event_name == 'pull_request' }}`, with a comment explaining that
branch pushes must never cancel each other for exactly this reason. **That guard works as
written.** It governs *in-progress* runs. GitHub permits only one *queued* run per concurrency
group, so a queued run is always evicted by a newer one regardless of the flag — and on this
branch nothing ever reaches in-progress to be protected.

The actual bottleneck is **GitHub-hosted macOS runner concurrency**, made catastrophic by the
lane-branch `push` trigger added 2026-07-27 (`ci.yml:26-30`) so a lane could obtain a CI-built
binary for its own changes:

- Every `CI` run schedules **three macOS jobs** (`Build (aarch64-apple-darwin)`,
  `Build (x86_64-apple-darwin)`, `CI (macos-latest)`).
- **30 runs were queued simultaneously**, 28 of them for lane branches already merged into
  `15cda12d` (verified by ancestry against the `gh` remote, with a decoy that discriminates).
- Observed queue depth: run `30414055173` (created 01:26Z) did not start its
  `Build (x86_64-apple-darwin)` job until **05:28Z — four hours later**. Run `30393604729`
  (created 2026-07-28T19:49Z) was still waiting on the same job ~10 hours in.

Merges land faster than the macOS queue drains, so every integration-branch run is evicted
while still queued. The self-hosted Windows runners are **not** the constraint — both are
`online`, and their jobs on those runs completed hours ago.

## 3. Action taken

**28 stale queued runs cancelled** (all for lane branches confirmed merged into `15cda12d`).
This frees the macOS queue immediately. Nothing product-side was touched; cancelled runs are
re-runnable.

Verification note: the first ancestry check reported every lane `UNMERGED` — because the refs
were on the `gh` remote, not `origin`, so `merge-base` failed and returned non-zero for all of
them. **A uniform answer with no known-positive is the self-passing shape, not a result.** The
re-measurement fetched `gh`, and its decoy (`lane/24-media-live`, genuinely unmerged) correctly
reads UNMERGED, so the instrument discriminates.

## 4. Still open

Cancelling the backlog treats the symptom. The lane-branch `push` trigger means **every lane
push costs three macOS jobs**, and lanes run 5–8 wide. The trigger was added for a narrow
purpose — letting one lane obtain a macOS or Windows binary — and now denies the integration
branch its coverage as a side effect. That needs a narrower mechanism. **Assigned to
`lane/ci-macos-budget`.**

This is the third distinct shape of the same structural problem in five days: CI dark because
of four clippy lines, then CI reporting on a 16-day-old `main`, now CI starved out of its own
queue. **The instrument keeps failing in a new way each time it is repaired**, which is the
argument for gating on it rather than around it.
