# 23A-C1-ATOMIC — lane notes

Running record, appended after every measurement (LANE-BRIEF §6b-i). Committed early and often
so a mid-lane death resumes from the last measurement, not from zero.

- Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23a-c1-atomic`,
  branch `lane/23a-c1-atomic`, base HEAD `bd5ba2259ecc4c936cdb30ab95a29ca34869b3b0`
  (= `lane/grade-23a`, the verdict lane). `git rev-parse --show-toplevel` resolves to the lane
  path, NOT `/Users/seandonahoe/dev/waylandcore`. Merge-base with `plan/f20-unified-audit-repair`
  = `861d1b1a716240165209336b1fa38d36f9445716`, captured once.

## T+0 — the job, from `23A-PHASE-VERDICT.md` (today's authority)

SC-1 = *"Generated skills cannot execute before governed promotion and can be observed, revoked,
and rolled back."* Four-clause conjunction. Verdict at base: (a) MET-WITH-STATED-EXCEPTIONS,
(b) PARTIAL, (c) PARTIAL, (d) PARTIAL → **NOT MET**.

Mine to close: **F23A-C1-H3** (HIGH, unfixed at base) plus clauses (b) and (c).

## T+0 — M1: F23A-C1-H3 CONFIRMED IN SOURCE, read directly, not inherited

`crates/wcore-skills/src/govern.rs` @ `bd5ba225`, 566 lines:

- `rollback()` :290. The restore is line **314**: `copy_tree(&payload, &record.source_dir)?;`
  — a bare recursive copy **straight into the live skills directory**.
- `copy_tree_inner` :523 is `create_dir_all` (:536) + per-file `std::fs::copy` (:561). No
  staging directory, no `rename(2)`, no `sync_all` anywhere on the restored tree.
- The only `sync_all` in the file is :454, inside `append_journal` — journal lines only. The
  only atomic write is `write_atomic` :500 → `wcore_config::atomic_write`, used at :262 and
  :267 for the two JSON records and **never for the payload**.

So a SIGKILL between the first and last `std::fs::copy` leaves `source_dir` present,
loadable, and missing content. The verdict's reading is correct.

Note the asymmetry that makes this a real defect rather than a stylistic one: `revoke()` is
carefully crash-ordered (module docs :26-43, steps 1-5) and *its* destructive step is last.
`rollback()` — the recovery verb, the one a user reaches for when something already went
wrong — has no such ordering at all.

## T+0 — M2: the two PARTIAL clauses, cause read from the verdict + source

- (b) `live_revocations()` :330, `journal()` :426, `is_revoked()` :370 all exist and are
  `pub`. Their only caller outside the crate is `crates/wcore-skills/src/bin/wcore-skill-govern.rs`,
  a dev-only auto-discovered bin. Verdict §2.c measured it in **zero** packaging manifests and
  zero workflows, with `wayland-core` at 31 hits in `release.yml` as the live control.
- (c) same: `revoke()` :221 is implemented and tested (`govern_revoke_rollback` 15 passed,
  `govern_cli_drive` 6 passed, per verdict §2.c) but reaches no shipped surface.

## T+0 — DECISION: surface via `/skill`, NOT via new `main.rs` flags

Recorded because it is a deliberate divergence from the obvious route.

The pending, unmerged lane `lane/23a-c1-governed` @ `3a2234d7` already adds
`--skills-revoke` / `--skills-rollback` / `--skills-govern` to `crates/wcore-cli/src/main.rs`
(verdict §3, dispatch :1566-1583). `main.rs` is a **LANE-BRIEF §6 shared-fence file**. Writing
the same four flags there in a second lane guarantees a merge conflict on the one file the
brief says to touch least, and buys nothing.

`crates/wcore-agent/src/slash/skill.rs` is **not fenced**, is already the phase's proven
observation surface for clause (b) — R7/R8 are LIVE-DRIVEN through it (verdict §2.b) — and is
reachable on the shipped binary *and* in the TUI. `wcore-agent` already depends on
`wcore-skills` (`Cargo.toml:21`), so no dependency-graph change.

So: `/skill govern`, `/skill revoke <name>`, `/skill rollback <id>`. If the pending lane also
merges, the two surfaces are complementary (non-interactive flags + interactive slash), not
duplicative, and they conflict in no file.

## STILL TO ESTABLISH

- [ ] Atomic restore lands and the existing `govern_revoke_rollback` suite stays green.
- [ ] **Kill mid-restore, for real.** Not a mocked failure: SIGKILL the process inside the
      restore window and grade the resulting directory as WHOLE-OLD / WHOLE-NEW / PARTIAL.
- [ ] The kill harness must have a **reddening control** — the same harness, same schedule,
      driving the pre-fix non-atomic copy, must report PARTIAL > 0. Without that, `PARTIAL=0`
      is the self-passing known-negative LANE-BRIEF §3b-i warns about.
- [ ] `/skill govern|revoke|rollback` live-driven on a real built binary, not just unit-tested.

## T+late — ORCHESTRATOR CORRECTION: rebased onto the train, two of three deliverables already done

Merged `gh/plan/f20-unified-audit-repair@4a872413`. `lane/23a-c1-governed@3a2234d7` IS an
ancestor of it (`git merge-base --is-ancestor` → rc=0), so the pending lane the verdict graded
separately has landed.

- **H3 already fixed in the train** — `rollback()` stages via `promote::staging_root_for` and
  publishes with `rename(2)`. My independent fix was the duplicate; dropped at the merge, taking
  the train's `govern.rs`, `loader.rs` and `govern_revoke_rollback.rs` verbatim.
- **Clauses (b)/(c) already surfaced** — `--skills-govern`, `--skills-revoke`,
  `--skills-rollback` on `main.rs`. My `/skill` verbs are complementary: measured the train's
  `slash/skill.rs` at 1 hit for `"list"` (control alive) and **0** for govern/revoke/rollback.

**So the remaining value was verification, and it paid.** Re-ran my harness against the train's
own implementation: `IN_WINDOW=29, ABSENT=28, WHOLE=1, PARTIAL=0`, with the legacy control at
27/28 PARTIAL. That is the independent re-execution `23A-PHASE-VERDICT.md` §3 said had not been
done, and it caught the WHOLE-after-kill branch my earlier run never reached.

And it exposed **F23A-C1-H4**: `staging_with_SKILL_md=28` of 29 killed restores. Cause and
consequence both measured with live controls, then fixed by a name fence in
`collect_skill_md`. Full record in `23A-C1-ATOMIC-KILL-EVIDENCE.md`; verdict in
`../../23A-C1-ATOMIC-SUMMARY.md`.

**Instrument defect, repaired not noted:** zsh expands `"$VAR:crates/…"` as the `:c` modifier,
which silently reported a present file as ABSENT; the redirect on the same line truncated
`govern.rs` to 0 bytes before the command failed. Caught by the known-positive control erroring
loudly. Always brace `"${VAR}:path"`, and check for damage after a failed `>` redirect.
