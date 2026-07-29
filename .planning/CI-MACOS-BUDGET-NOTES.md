# NOTES — lane/ci-macos-budget (working file, append-only)

Started 2026-07-29. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-macos-budget`,
branch `lane/ci-macos-budget`, merge-base `15cda12d`.

Assignment: repair CI starvation on `plan/f20-unified-audit-repair` without deleting the
lane-branch `push` trigger (which exists so a lane can obtain a CI-built macOS/Windows binary).

---

## Plan

1. **Re-measure the finding myself.** Do not inherit `.planning/CI-STARVATION.md`. Specifically
   establish, with live API evidence:
   - a. integration-branch `CI` runs since 2026-07-28T20:43Z are cancelled with `jobs.total_count == 0`;
   - b. the cancels correlate with a *newer run on the same concurrency group*, i.e. queued-run
     eviction, not the `cancel-in-progress` flag;
   - c. the macOS queue is the binding constraint — measure `created_at → started_at` latency
     per job, split by runner label (macos-latest vs self-hosted Windows vs ubuntu-latest).
   - **Instrument discipline:** every absence claim needs a known-positive in the same
     invocation. For (a) that means showing a run that DID get jobs (e.g. a `macOS native
     suites` run, or a pre-20:43Z CI run) through the identical query, so a uniformly-zero
     answer cannot come from a broken query. `gh` remote is GitHub; `origin` is a local path.

2. **Decide the mechanism.** Candidates (not prescribed):
   - restrict which *jobs* run on lane-branch pushes (the 3 macOS jobs are the cost);
   - opt-in signal for a lane that genuinely needs a Darwin/Windows artifact;
   - separate concurrency group for the integration branch so lane traffic cannot evict it;
   - collapse overlapping macOS jobs.
   Cross-audit the choice (codex / gemini / kimi + internal adversarial) before committing.

3. **Prove on the live system.**
   - integration-branch run *starting jobs* while lane pushes are in flight;
   - a lane push still obtaining a macOS/Windows artifact by the new route;
   - **counterfactual**: evidence the pre-change config would have evicted the run now shown
     surviving. A green run is not proof if it would have been green anyway.

4. Write `.planning/CI-MACOS-BUDGET.md` with the required frontmatter, commit, push to `gh`.

## Fences

Touching `.github/workflows/ci.yml` (+ possibly a new workflow file) and `.planning/*`.
No `crates/` changes expected → the two shared `wcore-cli` files should show zero diff vs
`15cda12d`. Will report fence exposure measured with `/usr/bin/git diff $BASE` where
`BASE=15cda12d`.

Not doing: merge to main, PR, tag, release, issue close, `wcore-contract generate`.
Will not cancel runs belonging to other live lanes (`24-media-live`, `24-media-bounds`,
`24-reconnect`, `24-msteams-attach`, `openapi-consumer`).

## Measurements

### M1 — starvation reproduced (integration branch), with a known-positive

`gh api repos/FerroxLabs/wayland-core/actions/runs/<id>/jobs --jq .total_count`, same query for
every row, taken 2026-07-29 ~05:40Z:

| run | branch | status/concl | jobs.total_count |
|---|---|---|---|
| 30425437335 | plan/f20… | pending | **0** |
| 30424714942 | plan/f20… | cancelled | **0** |
| 30423289490 | plan/f20… | cancelled | **0** |
| 30423121521 | plan/f20… | cancelled | **0** |
| 30421306254 | plan/f20… | cancelled | **0** |
| 30421332107 | plan/f20… | queued | **11**  ← known-positive |
| 30399974106 | plan/f20… | failure | **12**  ← known-positive |
| 30385909729 | plan/f20… | failure | **12**  ← known-positive |
| 30416430153 | macOS native suites | success | **1**  ← known-positive |

The instrument discriminates: the same call returns 11/12/1 for runs that did get runners. A
uniform zero would have been the self-passing shape; this is not that.

### M2 — the evictor is the branch's OWN pushes, NOT lane traffic (refutes one proposed fix)

`concurrency.group = CI-${{ github.ref }}` is **already per-ref**, so a lane run and an
integration run are in different groups and cannot evict each other. Measured directly:

- run 30424714942 (integration) created 05:18:59Z, cancelled **05:33:49Z**.
- lane runs created *inside* that interval: `lane/24-media-live` 05:29:44Z,
  `lane/24-msteams-attach` 05:32:17Z, `lane/24-gateway-surface` 05:17:11Z — **none cancelled it**.
- the integration push at **05:33:48Z** (run 30425437335) did, one second before the cancel.

Every row in the 40-run table shows the same 1-second coupling to the next *integration* run.
So "give the integration branch its own concurrency group" would change nothing — it already
has one. **The proposed option is refuted, not adopted.**

### M3 — macOS is the binding constraint (within-run control)

Run 30421332107, all jobs dispatched at the same instant (04:20:17Z):

| job | label | started | waited |
|---|---|---|---|
| CI (linux-containerized) | ubuntu-latest | 04:20:25Z | 8s |
| Build (x86_64-unknown-linux-gnu) | ubuntu-latest | 04:20:18Z | 1s |
| Build (aarch64-unknown-linux-gnu) | ubuntu-latest | 04:20:19Z | 2s |
| Build (x86_64-pc-windows-msvc) | windows-latest | 04:20:22Z | 5s |
| Build (aarch64-pc-windows-msvc) | windows-latest | 04:20:19Z | 2s |
| CI (Array) | self-hosted Windows | (queued) | — |
| CI (macos-latest) | macos-latest | 04:44:02Z | **24m** |
| Build (aarch64-apple-darwin) | macos-latest | **still queued at 05:40Z** | **>80m** |
| Build (x86_64-apple-darwin) | macos-latest | **still queued at 05:40Z** | **>80m** |

Run 30399974106 (dispatched 22:32Z): Linux/Windows all started within 6s; macOS started
**02:38Z / 03:35Z / 04:04Z** — 4h06m, 5h03m and **5h31m** later. Its last-finishing job
(`Build (x86_64-apple-darwin)`, 04:20:16Z) is followed **one second later** by the next
integration run's dispatch — that is the concurrency handoff, and it is gated on macOS.

Same run, same dispatch instant, every non-macOS label served in seconds. This rules out
account-wide throttling and isolates the macOS pool.

### M4 — capacity: peak 5 concurrent macOS jobs, ~11/hr throughput

**Instrument defect found and repaired in-lane (LANE-BRIEF §6b-ii).** My first interval sweep
included `conclusion=cancelled` macOS jobs. A cancelled job never runs but still carries an
`started_at`(enqueue) → `completed_at`(cancel) span, up to **368 min**. That reported
**61 concurrent** macOS jobs — which I nearly wrote down. Filtering to `success|failure`
(jobs that genuinely executed) gives **5**, exactly GitHub's standard hosted-macOS ceiling.

Repaired instrument carries a three-assertion self-test (all pass):
1. known-positive — two genuinely overlapping executed jobs → 2;
2. known-negative — two disjoint executed jobs → 1;
3. **the old matcher would have missed it** — on a fixture with one real job plus two
   never-ran cancelled jobs, old=3 vs new=1. Without (3) the self-test passes on the broken
   instrument too.

Window 2026-07-28T18:49Z → 2026-07-29T05:39Z (10.84 h), 303 CI runs:
- executed macOS jobs: **122**; duration min/median/p90/max = 12.7 / 19.6 / 26.8 / 33.8 min
- **peak concurrency 5**, throughput **11.25 executed macOS jobs/hour**

### M5 — demand split: 96% of macOS demand is lane traffic

243 macOS jobs created in the same 10.84 h window:

| | created | executed | still queued |
|---|---|---|---|
| `lane/**` | **234 (96.3%)** | 116 (94.3%) | 46 |
| `plan/f20-unified-audit-repair` | **9 (3.7%)** | **7 (5.7%)** | 2 |

Demand **22.4 macOS jobs/hr** vs capacity **11.25/hr** → **~2x oversubscribed, permanently**.
The queue grows ~11 jobs/hr and can never drain. The integration branch received **7 executed
macOS jobs in 10.8 hours** — about two runs' worth — which is why it has produced no verdict.

Live queue snapshot 05:40Z: **24 macOS jobs queued, 2 running**; 18 in-flight CI runs, 16 of
them lane branches.

### M6 — verdict on the handed-down finding

**HOLDS on its central claim, with one correction.** macOS runner scarcity is the cause and
the lane-branch `push` trigger is the multiplier — confirmed independently and more precisely
(96.3% / 2x oversubscription / ceiling of 5). The correction is M2: the finding's own
suggested remedy of a separate concurrency group for the integration branch would be inert,
because the group is already per-ref and lane traffic never evicts integration runs.

## Design (chosen after cross-audit — see CI-MACOS-BUDGET.md)

The lane trigger's stated purpose is a **CI-built macOS or Windows binary** for unmerged lane
work. Windows is NOT constrained (M3: 2-5s waits, plus a self-hosted runner), so the Windows
half of that capability survives untouched and unconditionally. Only the macOS half needs
rationing. Narrowing: on `lane/**` pushes, schedule the three macOS jobs **only on opt-in**
(`[ci-darwin]` / `[ci-macos]` in the head-commit message). `main`, the integration branch and
all pull requests keep the full matrix.

### M7 — first implementation measured and REVERTED (new finding)

The `budget` job + `needs:` form was implemented, pushed and measured. `ubuntu-latest` is
itself congested — live census 2026-07-29 ~06:10Z across 16 in-flight runs:

| label | completed | in_progress | queued |
|---|---|---|---|
| macos-latest | 4 | **5** | **36** |
| ubuntu-latest | 36 | 13 | **32** |
| windows-latest | 16 | 12 | 2 |
| self-hosted Windows | 4 | 0 | 11 |

macOS `in_progress == 5` independently re-confirms the ceiling from M4 by a completely
different method (live census vs interval sweep). And ubuntu congestion meant the budget job
sat **queued 14+ minutes** on run 30426418225 before `ci`/`build` were even created. Reverted
to a zero-job inline `strategy.matrix` expression.

### M8 — controlled before/after, and the live counterfactual

| arm | run | commit | config | jobs | macOS |
|---|---|---|---|---|---|
| A | 30425956850 | 27df8b7c | unmodified | 11 (12 incl. `report`) | **3** |
| B1 | 30427513255 | d8daf8e0 | this change, no token | **8** | **0** |

Same 22-minute window, integration branch still on the unmodified config, identical
`jobs.total_count` call: runs 30427396244 / 30426790209 / 30426629499 / 30426570745 /
30426416317 / 30426366364 all **jobs=0**. Known-positives in the same call: 12 and 8.

### M9 — artifact route proven end-to-end on this Mac

`gh run download 30399974106 -n wayland-core-aarch64-apple-darwin` →
`file` says `Mach-O 64-bit executable arm64`; `./wayland-core --version` → `wayland-core
0.12.25`, rc=0, on `uname -m = arm64`. Lane-branch route known-positive: run 30419996325 on
`lane/24-gateway-surface` carries `wayland-core-x86_64-apple-darwin`.

### M10 — arm B2: the opt-in restores full macOS coverage

Run 30427849371, commit cf7fc02e, an EMPTY commit whose only content is the token
`[ci-darwin]`. Dispatched 11 jobs including all three macOS jobs — identical to arm A under
the unmodified config. A→B1 isolates the change; B1→B2 isolates the token.

| arm | run | jobs | macOS |
|---|---|---|---|
| A  unmodified config      | 30425956850 | 11 | 3 |
| B1 change, no token       | 30427513255 |  8 | 0 |
| B2 change, `[ci-darwin]`  | 30427849371 | 11 | 3 |
