# BL-F26-SC3-O1-ROLLBACK — working notes (append-only, committed as I go)

Lane: `lane/restore-rollback-sqlite`
Base: `2c8b6d1d097e5bf7e2785dae349b5b6a31ea5160` (= `gh/plan/f20-unified-audit-repair`, SHA-asserted against `git ls-remote`).

## Minute 0-15 — what I have established by reading

### The defect site is located, and it is inside `journal.rs`

`crates/wcore-cli/src/backup/journal.rs`:

- `copy_inner()` L533-561 — the undo-store capture. L557 is
  `std::fs::copy(&from, &to)` per file, walked with `read_dir`, **each file read at
  whatever instant the walk reaches it**. This is byte-for-byte the same shape the
  archive lane proved corrupting in `backup create`.
- `preserve_scope()` L368-399 — L385 is the same `std::fs::copy` for the scoped
  (migrate) path.
- Both are reached from `OpGuard::preserve_target()` L209-217, which `restore --replace`
  calls before it clears the target.

So the answer to "can I close this without touching `journal.rs`?" is **no**, on
present reading: the raw copy *is* `journal.rs`. Intercepting upstream in `restore.rs`
would leave the defective primitive in place for every other caller of
`preserve_target`, which is the silent-degradation shape the brief forbids.
=> I must touch `journal.rs` and therefore must re-prove its interruption properties.

### Which interruption proofs depend on `journal.rs` (measured, not assumed)

Query run (unproxied), counting matches per file:
`/usr/bin/grep -on "backup recover\|\.wayland-journal\|undo-\|journal" <script>`

| proof script | journal refs | depends? |
|---|---|---|
| `scripts/portability-interrupt-proof.sh` (265 L) | yes, incl. `backup recover` L190 | **YES** |
| `scripts/portability-backup-rollback-sweep.sh` (373 L) | yes, incl. `backup recover` L205 | **YES** |
| `scripts/portability-migrate-rollback-proof.sh` (432 L) | yes (L101…L285) | **YES** |
| `scripts/portability-migrate-interrupt-proof.sh` (394 L) | **0** | no direct assert |

Instrument liveness for that grep: the same invocation returned non-zero counts for
three files and zero for the fourth, so it discriminates rather than returning a
free zero (LANE-BRIEF §3b-i).

Plus in-tree unit tests: `journal.rs`'s own `#[cfg(test)] mod tests` (L589+).

These three scripts are the re-proof set. Per the brief I must re-run them and report
what they returned, not assume a green suite covers them.

### What I inherit and must NOT reinvent

`crates/wcore-config/src/sqlite_snapshot.rs` (344 L) already provides:
- `is_sqlite_database()` — 16-byte header magic, content not filename;
- `is_derived_sidecar_of()` — length-equality matched against a KNOWN db name;
- `snapshot_database()` — `rusqlite::backup` single `step(-1)`, `integrity_check`
  verified before use, capture sidecars removed, **errors returned, never a fallback**.

Rejected-on-evidence, not to be revisited without new measurement:
`BEGIN IMMEDIATE`+byte-copy (PASSIVE checkpoint takes no writer lock);
`VACUUM INTO` (rebuilds from schema; `episodes_fts` is external-content FTS5 on an
implicit rowid).

### Open questions I still have to settle by measurement

1. Does the rollback path actually corrupt at base, under concurrent write load,
   through the real binary? **Must show RED first** — a green without a demonstrated
   red is worth nothing here.
2. `restore --replace` calls `preserve_target` and then `clear_tree_excluding_journal`.
   Who else calls `preserve_target`? (migrate's `ApplyGuard`.) Scope of blast radius.
3. Rollback restores the undo store with `copy_tree_all`. If the undo store now holds
   a *folded* capture (no `-wal`/`-shm`), rollback writes back a db with no sidecars
   while the live home may still have stale `-wal`/`-shm` from the pre-op process.
   **A stale sidecar next to a restored db is itself a corruption vector** — must
   handle sidecar removal on the restore leg, not just the capture leg.

## Minute 15-40 — the constraint resolves better than expected

### Q3 answered for the whole-tree path

`restore_from_undo()` L350-356 calls `clear_tree_excluding_journal(target)` **before**
`copy_tree_all(undo_dir, target)`. So the target is emptied first and no stale
`-wal`/`-shm` can survive a whole-tree rollback to sit beside a folded capture.
My worry #3 is closed for `restore --replace`. It is NOT closed for `restore_scope()`
L401-431, which removes only scope entries — but see below, no SQLite db is in scope.

### `MIGRATE_SCOPE` contains no database

`crates/wcore-cli/src/migrate/rollback.rs` L78-83:
`[quarantine, migrate-imported, skills, config.toml]`. No `memory.db`. So the scoped
capture leg reaches a database only if one is nested inside `skills/` or `quarantine/`,
which goes through `copy_tree_all` -> `copy_inner` and is covered by the same fix.

### The existing interruption proofs contain NO real SQLite database

Query (unproxied):
`/usr/bin/grep -in "sqlite\|memory\.db\|\.db\b" <the 3 proof scripts> <2 corpus gens>`
Instrument liveness in the same sweep: `grep -c "config.toml"` on the same file list
returned 4 / 2 / 1 / 0 — it discriminates, so the near-empty result below is a
measurement and not a free zero (LANE-BRIEF §3b-i).

Single hit across all five files:
```
scripts/portability-migrate-rollback-proof.sh:173:printf 'PRIOR-USER-MEMORY-DB\n' > "$TEMPLATE/memory.db"
```
That file is **named** `memory.db` and is a 22-byte text stub — it has no
`SQLite format 3\0` header. Because `sqlite_snapshot::is_sqlite_database()` decides by
**header magic and never by filename**, that stub is byte-copied exactly as today.

**Consequence: the fix is a no-op on every existing proof corpus.** The three
journal-dependent proofs should return exactly what they returned before. That is a
checkable prediction, not an assumption — I will re-run all three and report.

### The honest cost of the fix (must be stated, not buried)

Folding a WAL into the main file means a rolled-back SQLite database is a consistent
**equivalent database, not byte-identical bytes**. SC3's clause is "restore exact
pre-operation state", and for a live database that clause was already unsatisfiable —
the bytes being restored today are a torn mixture, i.e. not the pre-operation state
either. So this trades an *unachievable* byte-exactness for an achievable consistency,
and it must be named as an exception exactly as the archive lane named
`sqlite_captures` / `omitted_sqlite_sidecars` in the manifest.

### Fix shape decided

Capture legs snapshot; restore legs byte-copy. The undo store is quiescent by
construction, so re-snapshotting on the way back would be pointless work and would open
a read-write connection against a store that may be damaged. So `copy_inner` needs an
explicit direction, not a blanket "snapshot any db you see".

## Minute 60-140 — RED reproduced, fixed, re-proved

Binaries (`sha256sum`, unproxied, on hetzner):
- BASE `7cf8d45d8e4dad5c94cf5afd73dd6b3fea1083d32516b14a0d34af9c4bbb6972` @ 2c8b6d1d
- FIX  `a242cc99ddc9bdaeb812ba87926706a3f407038661ae8bfdae5c150eff35ee4f` @ 79087cd4

### RED at base — 7 FAIL / 1 PASS over 8 concurrent runs

First sweep (original instrument): c1 FAIL(101), c2 FAIL(100), c3 FAIL(101), c4 PASS.
Second sweep (repaired instrument): r1 FAIL(100), r2 FAIL(101) **and lost 16 rows
committed before the restore was launched**, r3 FAIL(1), r4 FAIL(1).
Controls (`--arm sequenced`, same writers stopped first): **2/2 PASS**, one demanding
65,914 rows — so the control is not vacuous and the harness can report a pass.

Two distinct corruption signatures, both with `restore` and `recover` exiting 0:
- `*** in database main ***` at 100-101 problem lines (SQLite's reporting cap);
- `wrong # of entries in index sqlite_autoindex_rows_committed_1`.

Every base run restored `['memory.db-shm', 'memory.db-wal']` into the home — the torn
trio put back beside the database, which is the mechanism.

**c4 PASSED at base.** Recorded rather than dropped: the defect is probabilistic, so a
single green run at base would have "disproved" it. That is exactly why the brief
demands a demonstrated red rather than a green.

### An instrument defect of my own, repaired in-lane (§6b-ii)

Run c3 reported `MISSING-COMMITTED-ROWS: -107`. Under `PRIMARY KEY (wid, n)` with
`n = 1..need`, `COUNT(*) WHERE n <= need` cannot exceed `need` — the corrupt btree
inflated the count. My accounting summed SIGNED differences, so such a surplus can
cancel a real loss from another writer.

Repaired: `account_rows()` clamps per writer and reports `IMPOSSIBLE-SURPLUS-ROWS`
separately, failing on it. `scripts/sqlite-restore-rollback-selftest.py` carries the
three assertions; A3 is the load-bearing one and it prints
`A3 verdicts on the same data: OLD=PASS NEW=FAIL` — the old matcher returned a clean
PASS on data that had lost 107 committed rows.

### GREEN with the fix — 4/4 PASS under HEAVIER load

f1-f4 all PASS, `ROLLED-BACK-SIDECARS: []`, control PASS (67,092 rows demanded).
The fixed arm is not an easier experiment: capture takes **2.21-2.77s** vs base's
**0.96-1.06s**, and the writers committed **~+3,700 each** across it vs **~+2,000** at
base. It passed under strictly more concurrency than the arm that failed.

### `journal.rs` WAS touched — and the re-proofs

Unavoidable: the raw `std::fs::copy` is `copy_inner`, inside `journal.rs`.

`cargo test -p wcore-cli --lib -- backup:: migrate::` →
`102 passed; 0 failed; 0 ignored; 0 measured; 1776 filtered out` (base: 98; +4 mine,
so all four compiled and ran — no silent skip).

**Known-negative:** reverting only `copy_tree_excluding_journal` to `SqliteMode::Verbatim`
makes 3 of the 4 new tests FAIL (verbatim output in the SUMMARY). The fourth,
`a_file_merely_NAMED_like_a_database_is_still_byte_identical`, passes in BOTH arms —
correct, since its job is to prove the change is a no-op for non-databases.

Re-proofs against the fixed binary:
| proof | result |
|---|---|
| `portability-interrupt-proof.sh` | `DIGEST-EQUAL: yes` / `PROOF-OK` |
| `portability-backup-rollback-sweep.sh` | `PROOF: PASS`, rollback-arm byte-diff 0, known-negative arm damaged 9/9 |
| `portability-migrate-rollback-proof.sh --peer openclaw` | `PROOF: PASS`, byte-diff 0, negative arm 6 |
| `portability-migrate-interrupt-proof.sh` | `PROOF: PASS peer=hermes trials=9 mid=9 recovered=9` |
| `portability-migrate-rollback-proof.sh --peer hermes` | **FAIL** — `only 1 of 9 kills landed mid-apply` |

The hermes failure is its own anti-vacuity guard firing on kill TIMING, not a
byte-identity failure (`ROLLBACK-ARM-BYTE-DIFF-UNEXPLAINED: 0`). Prior lane recorded
6/9 mid. Under investigation by A/B against the BASE binary on the same host — I will
not report it either way until the control says whether it is my change or the host.

## Status

A/B (3x base, 3x fix, hermes) running.
