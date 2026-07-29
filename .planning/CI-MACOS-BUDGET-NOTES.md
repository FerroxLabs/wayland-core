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

(appended below as they are taken)
