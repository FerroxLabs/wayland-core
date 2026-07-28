# 28-E5-REPAIR — running notes (append-only, re-committed after every measurement)

**Lane:** `lane/28-e5-repair` · **Started:** 2026-07-29 · **Base (merge-base):** `d3e871a0`

Goal: close ONE evidentiary hole — the `F-28-02-002` sandbox repair has never been exercised by
an E5 matrix run on any platform. Run the E5 Windows sandbox cells against a binary that
contains it, on `seandesktop`. This does NOT re-certify Phase 28.

---

## 1. Premise check (step 1 of the brief) — CONFIRMED

The repair commits, per `28-DRIFT.md` §1a:

```
15821c03  fix(sandbox): reclaim stale AppContainer ACL leases instead of wedging   (F-28-02-002)
3f3f93dc  fix(sandbox): extract the reclamation report so its wording stays pinned
9c4d2612  style(sandbox): rustfmt the 0-byte lease tests
```

### 1a. Every E5/soak candidate predates the repair

`git merge-base --is-ancestor`, run in this worktree:

| repair commit | ancestor of `32e2f57d` (28-02 matrix candidate) | ancestor of `e4a3f5fc` (28-03 soak candidate) | ancestor of lane HEAD |
|---|---|---|---|
| `15821c03` | **NO** | **NO** | yes |
| `3f3f93dc` | **NO** | **NO** | yes |
| `9c4d2612` | **NO** | **NO** | yes |

### 1b. Candidate bindings in the evidence

| artifact | `candidate.commit` |
|---|---|
| `evidence/28-01/candidate.json` | `32e2f57d…` (marked `provisional: true`) |
| `evidence/28-02/candidate.json` | `32e2f57d…` |
| `evidence/28-02/results.json` (the 651-cell E5 matrix) | `32e2f57d…` |
| `evidence/28-03/candidate.json` | `e4a3f5fc…` |
| `evidence/28-03/soak.json` | `e4a3f5fc…` |

`soak.json` is the only file of that name in the tree. It names `e4a3f5fc` and nothing else.
So the brief's phrasing ("every `candidate/results/soak.json` names only the two pre-repair
commits") holds, and it is exactly two commits, not more.

### 1c. Where the repair HAS been exercised (so the hole is precisely this shape)

`grep -rl '15821c03'` over the phase shows the repair appears in `28-h2` evidence
(`repro-after.log`, `unittest-after.log`, both `SRC_SHA=15821c035f14…`) — i.e. the targeted
live repro harness and the unit tests, on real `seandesktop` hardware. It appears in `28-adj`,
`28-adj2`, `28-drift` prose and in `28-04/findings.tsv` / receipt SUPERSEDING-002 as a
*disposition reference*. It appears in **no** matrix or soak results file.

**Premise verdict: TRUE as stated.** No E5 matrix run has ever exercised the repair.

---

## 2. Still to establish

- [ ] Reproduce the original E5 Windows invocation (`scripts/f28-native-matrix.mjs` +
      `evidence/28-02/f28-win-matrix.ps1`), not an approximation.
- [ ] Build a Windows candidate at a commit containing `15821c03`.
- [ ] Run the Windows sandbox-dimension cells; read back executed counts and real exit codes
      via the `WLRC=`/`WLDONE` status-file pattern (ssh+PowerShell collapses every non-zero to 1).
- [ ] Report honestly in either direction.
