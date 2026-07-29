# 26-SC3-ROLLBACK — working notes (append-only, committed as taken)

Lane `26-sc3-rollback`. Base `lane/grade-26`, later merged onto
`gh/plan/f20-unified-audit-repair @ 4a872413` per orchestrator correction.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-26-sc3-rollback`.

**SC3 (verbatim):** *Backup, restore, profile migration, and reciprocal portability
survive interruption and restore exact pre-operation state on rollback.* Graded
**PARTIAL** by `26-PHASE-VERDICT.md`.

---

## M0 — what the verdict established, and what it left open (minute ~15)

| noun | verdict grade | what was open |
|---|---|---|
| backup | MET | — (but no interruption arm of its own) |
| restore | MET (Linux + real Windows, `TerminateProcess`, `DIGEST-EQUAL: yes`) | single kill point only |
| profile migration | interruption MET, **rollback NOT** | `migrate` has NO rollback |
| reciprocal portability | same as migration | same |

Named gap **G3**: *"Rollback for `migrate` — a journal + reverse-apply so an
interrupted migration restores the pre-operation home."*

## M1 — the backup/restore journal, read in source (minute ~20)

`crates/wcore-cli/src/backup/journal.rs` is a genuine write-ahead journal: durable
intent before the first mutation, whole-tree undo store, dead-owner-only recovery
(the #181 rule), idempotent, prunes itself so a recovered tree digests identically.

`restore.rs:97` was the ONLY `journal::begin` call site in the workspace
(`/usr/bin/grep -rn --include='*.rs' "journal::begin" crates/` → 1 hit; the same
grep for `journal::recover` → 1 hit, `backup/mod.rs:246`, the manual
`backup recover` subcommand).

Restore writes into the **live** target — `clear_target()` then per-payload
`atomic_write` — the same shape as `F23A-C1-H3` in `wcore-skills`. Unlike that
one it is covered by a whole-tree undo store captured before the first mutation,
so the window is closed by the journal rather than by an atomic swap.

## M2 — the migrate write set, read in source (minute ~25)

`apply_plan` wrote exactly two things: `QuarantineStore::admit()` once per
executable item (incremental, interruptible) and `patch_global_config()` once at
the end (atomic). **No journal, no undo store, no reverse-apply.** Verdict
re-derived independently.

---

# FINDINGS

## F26-SC3-H1 — HIGH — a stale undo store clobbers a later COMPLETED restore

**FIXED. Proven failing at base first.**

`restore_archive` opened its own journal without settling records left by a dead
owner. Sequence a real user produces, because `backup recover` is a command they
have never heard of:

1. restore #1 → record A, undo-A = the pristine home. **SIGKILL mid-write.**
2. The user runs the restore again. It **succeeds**. `commit()` removes record B
   and undo-B, then `prune_journal_root` finds undo-A still present, so the
   journal root survives with record A + undo-A intact.
3. Any later recovery pass finds record A, owner dead, `preserved: true` →
   clears the target and restores the pristine tree, **destroying the content of
   the successful restore**.

Measured at base (`32bf7478`, hetzner, `cargo test -p wcore-cli --lib
backup::restore::`): `8 passed; 1 failed; 0 ignored; 0 measured; 1837 filtered
out` — the new test failing on exactly this assertion.

**Fix:** recovery runs before `journal::begin`, and before the OCCUPANCY
decision. The ordering matters twice: an interrupted restore clears the target
first, so its wreckage looks EMPTY, and judging occupancy against it let a plain
`restore` — no `--replace` — past the refusal that protects a live home.

## F26-SC3-H2 — HIGH — the digest counted Wayland's own bookkeeping as home state

**FIXED. Found by the live proof, not by inspection.**

The first passing-instrument run of the migrate rollback proof reported every
rolled-back home as differing from PRE. `diff -rq` against the template showed
**exactly one difference**: `.dirty-death.<pid>`, the crash-sentinel marker the
SIGKILLed process left behind. Every user file was byte-identical.

The digest is the comparand SC3's "exact pre-operation state" is judged on, so
bookkeeping inside it makes an exact rollback report as inexact — and the only
way to reach a green would have been to weaken the assertion. Excluded through
the same predicate as the journal directory, importing `crash_sentinel`'s
constants rather than respelling them.

The proof now also runs an **independent `diff -rq`** with a two-name allowlist,
so the digest's exclusion list cannot become a place to hide a difference.

## F26-SC3-M1 — MEDIUM — `dir_holds_state` counted the journal directory

**FIXED.** A home that was empty, whose restore was then killed, afterwards "held
state", so the retry was refused until the operator passed `--replace` to
overwrite nothing at all. It also meant the occupied-target refusal was surviving
an interruption only by accident.

## F26-SC3-S1 — SCOPE DRIFT caught by the probe, within hours of the probe existing

`MIGRATE_SCOPE` began as two entries because that WAS the write set. Merging
`gh/plan/f20-unified-audit-repair` brought `migrate/content.rs`, which imports
data skills into the **live `skills/` directory** and stages personas and memory
notes under `migrate-imported/`. Neither was in the scope; a rollback would have
reported success while leaving those writes behind.

`skills/` is a live user directory — the same shape as `F23A-C1-H3`. The drift
test now asks each store where it writes rather than restating paths.

## INSTRUMENT DEFECT — `$!` named the subshell, not the product

**Repaired in this lane, per LANE-BRIEF §6b-ii.** The first harness backgrounded
a shell *function*, so `$!` was the subshell's pid; `kill -9` killed the wrapper
and the product ran to completion. 27 of 27 trials measured a fully-imported
home. It was visible only because the byte-identity gate was strict — a gate
reading "the home is not corrupt" would have PASSED having interrupted nothing.

Repair: background the product directly, plus a **PID-TARGETING-CONTROL** that
measures BOTH launch mechanisms and fails if it cannot tell them apart (the third
assertion §6b-ii requires: the broken version must fail the new check).
Classification also moved from a digest comparison to an **open journal record** —
`config.toml` embeds the absolute home, so two homes never digest equal even
after identical complete imports.

---

# LIVE EVIDENCE (hetzner-dsm, release binary, merged tree)

All four nouns, multi-point uncatchable `SIGKILL`, byte-identity against a
captured pre-operation state. Three arms each: the property, a known-negative
with rollback removed, and a no-kill control.

| proof | mid-apply kills | interrupted → byte-identical | known-negative differed | no-kill recovered | verdict |
|---|---|---|---|---|---|
| migrate hermes | 7/9 | 9/9 | 4/4 | 0 | **PASS** |
| migrate openclaw | 6/9 | 8/8 | 5/5 | 0 | **PASS** |
| backup + restore | 9/9 | 9/9 | 9/9 | 0 | **PASS** |

`BYTE-DIFF-UNEXPLAINED: 0` on every arm — the independent `diff -rq` found no
difference beyond the two allowlisted bookkeeping names.
`backup create`: 9 kills, source home moved 0 times, 0 unverifiable archives.
