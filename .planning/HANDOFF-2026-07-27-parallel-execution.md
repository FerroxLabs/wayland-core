# HANDOFF — Wayland Core, parallel execution (2026-07-27)

Supersedes the operating sections of `HANDOFF-2026-07-26-autonomous-execution.md`. That file's
§4 (panel traps) and §5 (environment traps) are still current — read them.

Repo: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`.
Remote is `gh`. **NEVER touch `/Users/seandonahoe/dev/waylandcore`** — dirty, 642 files of drift.

---

## 1. Standing rules (all in `AGENTS.md` §11, all established by measurement)

1. **Live testing ranks at least as high as green code.**
2. **Lint plan gates to 0 HIGH** — `python3 .planning/scripts/lint-plan-gates.py <dir>`.
3. **Decide, do not park.** Checkpoints are cross-audited, not escalated. All 18 converted.

Reserved to Sean: **main merge, PR, tag, release, issue closure, deleting a retained evidence
ref, real credentials.** Nothing else. Pushing to this working branch is expected.

---

## 2. State

| Phase | State |
|---|---|
| 20 / 20A | COMPLETE. 20A at 13/15 REQ-native (r12, r13 open) |
| **21** | **Executed. Goal NOT ACHIEVED — twice, and the regrade was *worse*.** That is correct: the harness defects were repaired, so the corpus began measuring honestly, and the product does not enforce the envelope. All four F21 requirements OPEN. |
| 22, 23A, 23B, 24, 25, 27 | **PARTIAL** — roughly 1 plan of 4 each. 39 commits merged. Every phase graded itself honestly; not one claimed an unearned green. |
| 26 | **NOT STARTED** — its agent died on an API error. Restart from zero. |
| 28, 29, 30 | Not planned, deliberately — they certify/package/score what 24–27 build |

32 plans, **682 gates at 0 HIGH**.

---

## 3. Phase 21 product defects — the real output

Six HIGH defects the corpus found once it stopped being vacuous. **Three fixed and merged:**

- **F21-04-03 (`1eb9b5ca`)** — parallel `Spawn` siblings permanently faulted their budget
  authority. Root cause was a plain **TOCTOU** in `BudgetAuthorityCoordinator::build_and_append`
  (non-atomic read-modify-write), *not* Windows-specific and *not* contention — the window's
  width is set by I/O speed, which is why it measured 3/8 Linux vs 6/6 Windows. Reproduced
  **13/24 Linux, 23/24 Windows; both now 0/24.** This was blocking Phase 22's fleet supervision.
- **F21-02-01 (`6b0083b0`)** — child tool registries were never intersected with parent
  authority. Worse than stated: a bare `Delegate` granted Read/Grep/Glob unconditionally, so a
  `Full`-posture channel-remote parent — which has Grep/Glob dropped *because* recursive scan
  escapes the jail — handed exactly that back to its child. Proven live: child advertised
  `[["Bash"],["Bash"]]` while the parent held `[Delegate, Read]`.
- **F21-04-02 (`e879206e`)** — disproved with counter-evidence; the corpus binding was at fault.

**F21-02-03 (PolicyGate unreachable) is HELD BACK**, branch `fix/F21-02-03`. It collides with
F21-02-01 in `bootstrap.rs` (~2575) and `spawner.rs` (~975): both independently wired
parent→child authority at the same seam with different mechanisms (narrow-only
`ParentToolAuthority` cell vs `PolicyGate::from_parent_tools`). **Do not resolve by taking a
side** — that can silently drop one fix's seam coverage on the child-authority boundary. A
reconciliation agent is running; if it did not finish, re-dispatch with the same framing.

Still open: **F21-04-01** (no per-child observable on the host protocol — needs a FENCED
protocol seam request, and a `minor` bump is a coordinated release).

---

## 4. Orchestration — two errors, both fixed, do not repeat

**`isolation: 'worktree'` builds from the SESSION's git root**, which is
`/Users/seandonahoe/dev/waylandcore` — the forbidden repo. Seven agents were dropped into a tree
with no `.planning/`. Instead, instruct each agent to create its own:

```bash
git -C /Users/seandonahoe/dev/waylandcore-ferrox worktree add -b <branch> \
    /Users/seandonahoe/dev/waylandcore-frontier-worktrees/wt-<id> plan/f20-unified-audit-repair
```
and to verify `git rev-parse --show-toplevel` before doing anything.

**Do not run 7+ full-workspace Rust builds on hetzner at once.** That filled the disk and made
sshd stop answering for hours. Cap at ~5, use targeted `-p <crate>`, give each agent its own
`/root/wayland-<id>` worktree, and treat a connection timeout as **load to back off from**, not
a dead host.

**Known tooling bug:** `.planning/rescue/*.patch` are lossy human-readable summaries, not
applyable diffs — `git apply` rejects them. The live worktrees are the real source. Fix the
rescue capture to use `git diff HEAD > f.patch` verbatim.

---

## 5. hetzner-dsm — ~965G reclaimed, root cause fixed

Was 95% full / 84G free. **Now 56% / 749G free.**

- **Root cause (`6ae6f623`): no `[profile.dev]` existed**, so cargo's default applied full DWARF
  to every crate *and dependency* — >1GB test binaries, 314G in one `target/`, 190,330 files.
  Now `debug = "line-tables-only"` + `debug = false` for deps. **Measured 1.3G → 655M (50%)**,
  and backtraces still resolve to file:line:column.
- Finished agents leave warm `target/` dirs (~392G worth). Reap them once their work is merged.
- Docker had **110GB build cache with zero active** — pruned. Images/volumes untouched.
- **`/root/rambuild` is a 150G tmpfs holding Docker's data root** — 128G of RAM, which is why a
  251G box felt starved. Not ours; `dockerd`/`buildkitd` hold it open.
- Swap was fully exhausted; cleared with `swapoff -a && swapon -a` (needs free RAM ≥ swap).
- Still available if needed: `/root/wayland/target` (302G, in use), ~80–100G of dead Flux
  Docker images/volumes (**Sean's call — Flux production**).

---

## 6. Next

1. Finish the F21-02-03 reconciliation and merge.
2. Re-run the unfinished plans for 22, 23A, 23B, 24, 25, 27 (most are 3 of 4 outstanding).
3. **Phase 26 from zero.**
4. F21-04-01 protocol seam request.
5. Then 28 → 29 → 30.

Verify what landed before redoing anything — agents died mid-write repeatedly today, so partial
state on disk is the norm, not the exception.
