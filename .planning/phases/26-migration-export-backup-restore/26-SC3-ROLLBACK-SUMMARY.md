---
phase: 26
plan: SC3-ROLLBACK
subsystem: portability / backup / migrate
lane: 26-sc3-rollback
branch: lane/26-sc3-rollback
base: lane/grade-26, merged onto gh/plan/f20-unified-audit-repair @ 4a872413
criterion: "SC3 — Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback"
prior_grade: PARTIAL (26-PHASE-VERDICT.md)
verdict: "SC3 MET on Linux for all four nouns; Windows leg NOT executed"
findings: "F26-SC3-H1 HIGH (fixed) — a stale undo store clobbers a later COMPLETED restore; F26-SC3-H2 HIGH (fixed) — the home digest counted Wayland's own bookkeeping as user state; F26-SC3-M1 MEDIUM (fixed) — dir_holds_state counted the journal directory; F26-SC3-S1 (fixed) — scope drift caught by the probe; F26-SC3-O1 MEDIUM (open, BACKLOG) — SQLite WAL sidecars archived as three independently-read files; F26-SC3-O2 MEDIUM (open, BACKLOG, PRE-EXISTING) — migrate_hermes idempotence test flaky at base"
fences: "clean — no crates/wcore-cli/src/lib.rs, no main.rs, no .github/"
status: complete
---

# Phase 26 SC3 — Rollback: exact pre-operation state under real interruption

## Verdict

**SC3 is MET on Linux for all four nouns.** The clause the verdict graded PARTIAL —
*restore exact pre-operation state on rollback* — was unmet by `migrate`, which had
no rollback at all. It has one now, and all four nouns are proven by multi-point
uncatchable `SIGKILL` against a captured pre-operation state.

**What I did NOT do:** no Windows leg (gap G4 stays open), no real peer home was
interrupted (the corpora are synthetic), and I did not touch SC2's import/export or
quarantine paths, nor `wcore-skills/src/govern.rs`.

| noun | before | after |
|---|---|---|
| **backup** | interruption implicit; no arm of its own | 9 kills swept across a measured 58 ms window; source home moved **0** times; **0** unverifiable archives published |
| **restore** | exact rollback proven at ONE kill point | **9/9** kills mid-flight, **9/9** homes byte-identical, 0 unexplained byte diffs |
| **profile migration** | interruption met, **rollback absent** | journal + reverse-apply; **7/9** mid-apply kills, **9/9** interrupted homes byte-identical |
| **reciprocal portability** | same as migration | openclaw peer: **6/9** mid-apply, **8/8** byte-identical |

---

## The interruption points I actually killed at

Every trial is a real `SIGKILL` — uncatchable, no handler, no `atexit`, no `Drop` —
at a delay swept across the operation's window **measured on the hardware**, not
guessed. Mid-flight is classified by an **open journal record**, which states
directly that the process died inside the window the journal covers.

- **restore**, 9 points from 339 ms to 3051 ms across a 3.4 s window: inside
  `clear_target` (the home at its emptiest), first payload, mid-payload-loop, last
  payload, and the credential rewrite. 9/9 landed mid-flight.
- **migrate**, 9 points from 19 ms to 193 ms across a 215 ms window: inside
  `preserve_target`, inside the `admit` loop, at the index rewrite, and around the
  single atomic `config.toml` write. 7/9 (hermes) and 6/9 (openclaw) landed
  mid-apply; the rest landed pre-apply or post-completion and are graded as such.
- **backup create**, 9 points across a measured 58 ms window: 6 before publication,
  3 after. The three that published produced archives that **verify**.

## The byte-identity evidence

Two independent comparisons, because one would not be enough:

1. **The product's own digest** (`backup digest --home`), the same arithmetic the
   journal records — so both sides of the comparison are computed identically and on
   every platform.
2. **`diff -rq` against the template with no exclusions**, allowlisting exactly two
   bookkeeping names. `BYTE-DIFF-UNEXPLAINED: 0` on every arm of every proof.

The second exists because the digest deliberately excludes Wayland's own
bookkeeping, and an exclusion list is otherwise a place to hide a difference.

**Every proof carries a known-negative arm** — the identical kills with the rollback
removed — which must leave the home DIFFERENT from PRE. It did, every time:
4/4, 5/5 and 9/9 mid-flight kills damaged the home when nothing rolled it back. Had
that arm reported "identical", the property arm would have proven nothing and the
run FAILS on that gate. A third `nokill` arm proves recovery does **not** revert a
completed operation: 9/9 completed, 0 recovered, 0 reverted, on all three proofs.

---

## Findings

### F26-SC3-H1 — HIGH, FIXED — a stale undo store clobbers a later COMPLETED restore

`restore_archive` opened its own journal without settling records left by a dead
owner. The sequence a real user produces, because `backup recover` is a command they
have never heard of:

1. restore #1 → record A, undo-A = the pristine home. **Killed mid-write.**
2. The user runs the restore again. It **succeeds**. `commit()` removes its own
   record, then `prune_journal_root` finds undo-A still present, so the journal root
   survives with record A and undo-A intact.
3. Any later recovery pass finds record A, owner dead, `preserved: true` → clears the
   target and restores the pristine tree, **destroying the successful restore**.

**Proven failing at base before the fix** (`32bf7478`, hetzner):
`8 passed; 1 failed; 0 ignored; 0 measured; 1837 filtered out`.

Recovery now runs before `journal::begin` **and before the occupancy decision**. The
second half matters independently: an interrupted restore clears the target first, so
its wreckage looks EMPTY, and judging occupancy against it let a plain `restore` —
no `--replace`, the mode whose whole contract is "I will not overwrite a home that
has something in it" — past the refusal and on to overwrite the live home.

### F26-SC3-H2 — HIGH, FIXED — the digest counted Wayland's own bookkeeping

Found by the live proof, not by inspection. The first run on a repaired instrument
reported every rolled-back home as differing from PRE. `diff -rq` showed **exactly
one** difference: `.dirty-death.<pid>`, the crash-sentinel marker the killed process
left behind. Every user file was byte-identical.

The digest is the comparand SC3 is judged on, so bookkeeping inside it makes an exact
rollback report as inexact — and the only route to green would have been to weaken
the assertion. Excluded through the same predicate as the journal directory,
importing `crash_sentinel`'s constants rather than respelling them.

### F26-SC3-M1 — MEDIUM, FIXED — `dir_holds_state` counted the journal directory

A home that was empty, whose restore was then killed, afterwards "held state", so the
retry was refused until the operator passed `--replace` to overwrite nothing at all.

### F26-SC3-S1 — scope drift, caught by the probe within hours of the probe existing

`MIGRATE_SCOPE` began as two entries because that WAS the write set. Merging
`gh/plan/f20-unified-audit-repair` brought `migrate/content.rs`, which imports data
skills into the **live `skills/` directory** and stages personas and memory notes
under `migrate-imported/`. Neither was in the scope; the rollback would have reported
success while leaving those writes behind. `skills/` is a live user directory — the
same shape as `F23A-C1-H3` in `wcore-skills`. The drift test now asks each store
where it writes rather than restating paths.

### Instrument defect, found and repaired in this lane (LANE-BRIEF §6b-ii)

My first harness backgrounded a shell **function**, so `$!` was the subshell's pid;
`kill -9` killed the wrapper and the product ran to completion. **27 of 27 trials
measured a fully-imported home.** It surfaced only because the byte-identity gate was
strict — a gate reading "the home is not corrupt" would have PASSED having
interrupted nothing.

Repaired, not merely noted: the product is backgrounded directly, and a
**PID-TARGETING-CONTROL** measures BOTH launch mechanisms and fails if it cannot tell
them apart — the third assertion §6b-ii requires, so the broken version fails the new
check. Classification also moved off digest comparison, because `config.toml` embeds
the absolute home and two homes never digest equal even after identical complete
imports.

---

## Open — reported, not fixed

### F26-SC3-O1 — MEDIUM — SQLite WAL sidecars are archived as three separate files

Prompted by the orchestrator's note on `lane/wal-nfs`. **Measured**, not assumed: a
home containing `memory.db`, `memory.db-wal` and `memory.db-shm` produces
`PAYLOAD-PATHS: ['config.toml', 'memory.db', 'memory.db-shm', 'memory.db-wal']` —
all three carried, each read at a different instant by the walk. A concurrent writer
can therefore yield a mutually inconsistent trio, and a restored `-shm` is derived
state tied to a dead process.

Not fixed: it needs a SQLite-aware quiesce (`VACUUM INTO` / `sqlite3_backup`) and
touches `wcore-memory`, outside this lane's four nouns. `crates/wcore-config/src/sqlite_journal.rs`
is the selector to build on. My paths open no SQLite connection and copy bytes only,
so nothing here reverts that lane's work.

### F26-SC3-O2 — MEDIUM, PRE-EXISTING — `migrate_hermes::import_is_idempotent_without_overwrite` is flaky

Full suite at my HEAD: **2168 passed, 2 failed, 5 ignored**. Both failures are
pre-existing:

- `always_fails` — the deliberately-panicking scaffolded fixture `26-GAPS-SUMMARY.md`
  named; the test is literally called that.
- `import_is_idempotent_without_overwrite` — asserts on the raw `config.toml` string,
  but `ConfigFile.profiles` is a `HashMap`, so section order shuffles between runs.
  The two sides of the failure differ **only** in whether `[profiles.alpha]` precedes
  `[profiles.beta]`; content is identical.

**Measured at base `4a872413` with none of my commits: 3 failures in 11 runs.** Not
introduced here. Left for BACKLOG per the severity policy — it is a test comparing a
non-deterministic serialization, not a product correctness defect. (The same
non-determinism is already documented inside `portability-migrate-interrupt-proof.sh`,
which normalises section order for exactly this reason.)

### Still open from the verdict's gap list

- **G4 — a Windows interruption leg for the migration path.** Not executed. The
  harnesses are POSIX `sh` with no PowerShell peer.
- **Synthetic corpora.** No real peer home was interrupted.
- **`ORPHAN-PAYLOAD-TRIALS: 5`** persists in the pre-existing forward-convergence
  proof — a payload can be written before its index entry exists. Every such trial
  recovers on re-drive, and it is now moot for rollback (the whole scope is reverted),
  but it is not "no orphans".

---

## Gate results (unproxied tools; hetzner-dsm, release binary)

```
cargo test -p wcore-cli --lib -- backup:: migrate::
    98 passed; 0 failed; 0 ignored; 0 measured; 1772 filtered out
cargo test -p wcore-cli (full)
    2168 passed; 2 failed; 5 ignored     [both failures pre-existing, above]
cargo clippy -p wcore-cli --all-targets   clean
cargo fmt --all -- --check                rc=0 (Mac)

portability-interrupt-proof.sh            PROOF-OK   DIGEST-EQUAL: yes   [regression, pre-existing]
portability-migrate-interrupt-proof.sh    PROOF: PASS peer=hermes mid=6 recovered=9   [regression, pre-existing]
portability-migrate-rollback-proof.sh --peer hermes     PROOF: PASS
portability-migrate-rollback-proof.sh --peer openclaw   PROOF: PASS
portability-backup-rollback-sweep.sh                    PROOF: PASS
```

Both pre-existing interruption proofs still pass, so the recovery-ordering change did
not regress the single-kill restore proof or migrate's forward convergence.

## Files changed

```
crates/wcore-cli/src/backup/journal.rs      scoped journal, absent-set, bookkeeping predicate
crates/wcore-cli/src/backup/restore.rs      recover-before-occupancy, injectable liveness seam
crates/wcore-cli/src/backup/mod.rs          dir_holds_state, recovered_before_start output
crates/wcore-cli/src/crash_sentinel.rs      two constants pub(crate) (no behaviour change)
crates/wcore-cli/src/migrate/mod.rs         apply wrapped in the journal guard
crates/wcore-cli/src/migrate/rollback.rs    NEW — ApplyGuard, scope, out-of-scope probe
scripts/portability-migrate-rollback-proof.sh   NEW
scripts/portability-backup-rollback-sweep.sh    NEW
```

**Fence: clean.** `git diff --name-only <merge-base> HEAD -- crates/wcore-cli/src/lib.rs
crates/wcore-cli/src/main.rs .github/` → **0**.

## For the orchestrator to serialize

- `crates/wcore-cli/src/crash_sentinel.rs` — two `const` become `pub(crate)`. Additive,
  no behaviour change, but another lane touching that file will conflict trivially.
- `crates/wcore-cli/src/migrate/mod.rs` — the guard wraps `apply_plan` inside
  `run_source`. Merged cleanly against `4a872413`; a lane editing the same block will
  need a look.
- No contract change, no PR, no merge, no tag.
