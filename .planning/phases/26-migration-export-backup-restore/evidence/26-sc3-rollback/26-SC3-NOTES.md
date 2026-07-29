# 26-SC3-ROLLBACK — working notes (append-only, committed as taken)

Lane `26-sc3-rollback`. Base `lane/grade-26`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-26-sc3-rollback`.

**SC3 (verbatim):** *Backup, restore, profile migration, and reciprocal portability
survive interruption and restore exact pre-operation state on rollback.* Graded
**PARTIAL** by `26-PHASE-VERDICT.md`.

---

## M0 — what the verdict already established, and what it left open (minute ~15)

Re-read from `26-PHASE-VERDICT.md` "Criterion 3", not inherited as fact — each claim
below is re-derived in source in M1/M2.

| noun | verdict grade | what is open |
|---|---|---|
| backup | MET | — |
| restore | MET (Linux + real Windows, `TerminateProcess`, `DIGEST-EQUAL: yes`) | — |
| profile migration | interruption MET, **rollback NOT** | `migrate` has NO rollback; converges forward on re-drive |
| reciprocal portability | same as migration | same |

Named gap **G3**: *"Rollback for `migrate` — a journal + reverse-apply so an
interrupted migration restores the pre-operation home."* That is the literal unmet
clause and it is my primary target.

Residual the verdict did not round off: `ORPHAN-PAYLOAD-TRIALS` still 9 (hermes) /
11 (openclaw) — a quarantine payload can be written before its index entry exists.

## M1 — the backup/restore journal, read in source (minute ~20)

`crates/wcore-cli/src/backup/journal.rs` (568 lines) is a genuine write-ahead journal:

- `begin()` writes a durable intent record BEFORE the target is touched, scoped per
  op AND per owning pid (the #181 rule).
- `preserve_target()` copies the ENTIRE prior tree into an undo store, then flips
  `preserved: true`.
- `recover()` / `recover_with()` roll back every record whose owner is **dead**,
  newest-first; skips live owners; idempotent; prunes the journal dir so a recovered
  tree digests identically to a tree that never carried one.
- `restore.rs:97` is the ONLY `journal::begin` call site in the workspace
  (`/usr/bin/grep -rn --include='*.rs' "journal::begin" crates/` → 1 hit).

So restore writes into the **live** target (`clear_target()` then per-payload
`atomic_write`) rather than staging-and-swapping — the same shape as the
`wcore-skills` `rollback()` defect `F23A-C1-H3` — but unlike that one it is
**covered by a whole-tree undo store captured before the first mutation**, so the
lost-update window is closed by the journal rather than by atomicity of the swap.

### D1 — SUSPECTED DEFECT: a stale undo store can clobber a later completed op

`restore_archive()` calls `journal::begin()` **without first recovering pending
dead-owner records**. Hypothesised sequence:

1. restore #1 → record A, undo-A = pristine tree. **SIGKILL mid-write.** Tree is now
   damaged (`clear_target` runs first, so it is near-empty).
2. User re-runs `backup restore --replace` (the natural thing to do; they do not know
   `backup recover` exists). Record B's `pre_digest` is the DAMAGED tree; undo-B is
   the damaged tree. Restore #2 **succeeds**; `commit()` → `close()` removes B, then
   `prune_journal_root` finds undo-A still present so the journal root SURVIVES with
   record A + undo-A intact.
3. Any later `backup recover` (or a third op's recovery) finds record A, owner dead,
   `preserved: true` → `restore_from_undo` **clears the target and restores the
   pristine pre-op tree**, destroying the successfully restored content of step 2.

If real this is user-facing data loss, severity HIGH. **Status: hypothesis from
source reading. Must be proven by an executable test that fails at base before any
fix.** Not yet proven — do not report it until it is.

### D2 — recovery is manual-only

`journal::recover` has exactly one call site in the workspace,
`backup/mod.rs:246` = the `backup recover --home` subcommand. Nothing calls it on
startup, and `restore_archive` does not call it. A user killed mid-restore is left
with a damaged home and no signal. Severity depends on D1.

## M2 — the migrate write set, read in source (minute ~25)

`migrate/mod.rs:597 apply_plan` production write set is exactly two things:

1. `QuarantineStore::admit()` — called **once per executable item**, each call writing
   payload bytes under `<home>/quarantine/payloads/…` and then rewriting the WHOLE
   index (`quarantine.rs:369 save_index` → `wcore_config::atomic_write`, the
   F26-GAPS-H1 fix, confirmed present at my base).
2. `patch_global_config()` — ONE call at `mod.rs:786`, at the very end, writing
   `f.profiles` and `f.mcp.servers` into `config.toml`.

So the operation is **N incremental live mutations followed by one atomic mutation**,
with **no journal, no undo store and no reverse-apply**. A kill inside the admit loop
leaves k of N items admitted and `config.toml` untouched — self-consistent, but NOT
the pre-operation state. That is precisely the verdict's finding, re-derived.

Bounded write set ⇒ a **scoped** journal is exact here: `<home>/quarantine/**` and
`<home>/config.toml`. Whole-tree preservation (as restore does) would copy `memory.db`
and every asset on every migrate, which is not acceptable for an import command.

**Ownership note:** lane `26-sc2-import` owns import/export + quarantine
classification, so I will journal at the `apply_plan` boundary in a NEW file and will
NOT edit `migrate/quarantine.rs` or `migrate/select.rs`.

## Plan

- P1. Prove or kill D1 with a test that fails at base. Fix if real.
- P2. Add a scoped operation journal + reverse-apply to `migrate` apply (G3).
- P3. Multi-point kill harness: capture pre-op digest, SIGKILL at several distinct
  windows, roll back, assert byte-identity. Include a **known-negative** run with
  rollback disabled that must FAIL, or the proof is self-passing.
- P4. Out-of-scope-write guard: digest everything outside the declared scope before
  and after, so "the write set is bounded" is a measurement, not a claim.
