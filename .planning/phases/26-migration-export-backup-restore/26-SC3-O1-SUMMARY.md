---
lane: backup-sqlite
defect: F26-SC3-O1
severity_at_open: MEDIUM (reclassified HIGH on measurement — silent restore corruption)
status: FIXED and live-proven
branch: lane/backup-sqlite
findings: "F26-SC3-O1 (fixed) — a live SQLite home archived as three independently-read files restores CORRUPT while create and restore both exit 0; F26-SC3-O1-I1 (fixed in-lane) — the proof harness published its own progress non-atomically and fabricated a stalled-writer reading"
---

# F26-SC3-O1 — `backup create` over a live SQLite home

## Verdict

**Fixed, and proven by a demonstrated torn read rather than an assertion.**
The defect reproduced 4/4 through the real `wayland-core` binary; the fix passes
6/6 under identical concurrency; and the control arm proves the harness can
report both outcomes.

I also reclassified the severity. It was filed MEDIUM. It silently hands the
user a corrupt database with `rc=0`, which is the same shape as the WAL-on-NFS
defect this programme graded HIGH. I did not change the label in the register —
that is a disposition decision, not mine — but I am recording the disagreement.

## The known-negative: a torn read, measured

`scripts/sqlite-backup-consistency-proof.py` drives the real binary while three
stock-Python `sqlite3` writers commit into a 158 MiB `memory.db`. Stock Python
deliberately: a reproduction whose writer is our own library invites the charge
that the failure was arranged.

| binary | arm | commits by each writer DURING the archive | restored sidecars | `integrity_check` | committed rows lost | verdict |
|--------|-----|-------------------------------------------|-------------------|-------------------|---------------------|---------|
| base `eaff921d` | concurrent | +23,131 / +23,654 / +23,776 | `-shm`, `-wal` | **corrupt**, 100 problem lines | 0 | FAIL |
| base | concurrent | +24,260 / +24,431 / +23,833 | `-shm`, `-wal` | **corrupt**, 101 | 0 | FAIL |
| base | concurrent | +21,558 / +21,885 / +21,853 | `-shm`, `-wal` | **corrupt**, 101 | **20** | FAIL |
| base | concurrent | +23,356 / +23,085 / +22,895 | `-shm`, `-wal` | **corrupt**, 101 | 0 | FAIL |
| base | **sequenced** (same writers, stopped first) | n/a | none | `ok` | 0 | **PASS** |
| fixed | concurrent x4 | ~+23,000 each | none | `ok` | 0 | **PASS** |
| merged | concurrent | +20,403 / +19,536 / +19,921 | none | `ok` | 0 | **PASS** |
| merged | sequenced | n/a | none | `ok` | 0 | **PASS** |

The corruption is `btreeInitPage() returns error code 11` (SQLITE_CORRUPT) across
100 pages — SQLite's reporting cap, so the true count is at least that. One run
also lost **20 rows that had been committed before the archive was even
launched**: not only structurally corrupt but lossy.

**`backup create` exited 0. `backup restore` exited 0. Nothing warned.**

### The control is the load-bearing half

The `sequenced` arm runs the SAME writers over the SAME rows and waits for every
one to EXIT before archiving: **64,141 rows demanded, all present,
`integrity_check=ok`**. Concurrency is the only variable between the two arms.
A `quiescent` arm with no writers would also have made the row-survival question
vacuous, leaving only `integrity_check` doing any work.

### Anti-vacuity guards, and one that fired

Per LANE-BRIEF section 6a-i the driver asserts, and ABORTS rather than passing:

1. every writer wrote a START marker — written only after its **first commit
   succeeded**, so process launch is not enough;
2. every writer's committed count **increased across the archive window** — an
   actor that was present but blocked is a dead instrument too;
3. the journal mode is read back from each writer's own marker, not inferred
   from the PRAGMA we sent (section 3b-ii);
4. a PASS in an arm that demanded zero rows is downgraded to
   `ABORTED-VACUOUS-NOTHING-WAS-DEMANDED`.

Guard 2 fired on the very first run. **The guard was right and my instrument was
wrong** — see below.

## An instrument defect, repaired in-lane

Run 1 reported `w1: 2820 -> 0` and aborted. The writer published its high-water
mark with `open(path,"w")`, which truncates before it writes; the driver read
that zero-byte window and `int(raw or "0")` turned it into a *number*. A writer
committing ~29,000 rows was reported as stalled.

Per LANE-BRIEF section 6b-ii I repaired the instrument rather than writing it up:

- publish via `os.replace` (atomic on POSIX and Windows);
- `read_progress` RAISES `HarnessError` on an empty or unparseable marker
  instead of substituting a plausible number.

`scripts/sqlite-backup-harness-selftest.py` carries the three required
assertions. The third is the one that proves the repair does anything: the OLD
publisher is retained and produces **28,777 observable partial reads** on
hetzner; the repaired one produces **0**.

## The design, and why this one

`crates/wcore-config/src/sqlite_snapshot.rs` — a sibling of the existing
`sqlite_journal.rs`, which was **not touched**. Detection is by the 16-byte
header magic, never by filename.

Each database is captured through SQLite's **online backup API**
(`sqlite3_backup_*`, `rusqlite::backup`) with a **single `step(-1)`**: the whole
database inside ONE read transaction, so it observes one WAL snapshot and cannot
see a half-applied checkpoint. The documented restart-on-external-write occurs
*between* successive `step()` calls; with one call there is no between. The
capture is then **verified** with `integrity_check` before it is packed, and the
`-wal`/`-shm`/`-journal` sidecars are dropped and NAMED in the manifest.

Failure **refuses the archive**. It does not fall back to a raw copy — that
would reinstate the defect silently, and worse, behind an archive now claiming
consistency.

### Panel, and why I did not simply follow the majority

The cross-audit split 2-1 (codex=A, gemini=A, kimi=B) **on facts, not
preference**, so I measured instead of counting votes.

- **Rejected: `BEGIN IMMEDIATE` + byte-copy the trio.** All three auditors
  converged that a PASSIVE checkpoint does not take the writer lock, so another
  connection can write pages into the main file mid-copy — the byte copy is not
  actually frozen. Kimi's proposed repair (a `wal_checkpoint(TRUNCATE)` from a
  second connection while the first holds `BEGIN IMMEDIATE`) **self-deadlocks**:
  TRUNCATE requires the very writer lock the first connection holds.
- **Rejected: `VACUUM INTO`.** It rebuilds from the schema, so it depends on
  every virtual-table module being registered (this workspace ships `vec0` from
  the loadable `sqlite-vec`) and on SQLite's documented "may change the ROWIDs
  of entries in tables that do not have an explicit INTEGER PRIMARY KEY".
  `wcore-memory` declares `episodes (id TEXT PRIMARY KEY)` with `episodes_fts`
  as an external-content FTS5 index keyed on that **implicit** rowid, so a
  renumbering would leave `integrity_check` clean while every full-text search
  returned the wrong rows.

  **I could not demonstrate that renumbering.** `scripts/sqlite-snapshot-primitive-probe.py`
  reproduces exactly that schema with rowid gaps and shows VACUUM INTO
  preserving both rowids and the FTS mapping at SQLite 3.53.2. So VACUUM INTO is
  rejected for resting on a documented "may" and on module registration — not
  for an observed failure. Saying otherwise would be claiming evidence I do not
  have.

The chosen primitive needs neither: it copies pages and never executes the
schema, so the vec0 question and the rowid question both stop existing.

### What the fix does NOT promise

The captured bytes are a consistent **database**, not a byte-identical **file** —
folding a WAL into the main file is the point, so it cannot be. The module's
documented byte-exact round trip therefore now has a second named exception
beside `absent_secrets`, and the manifest records it: `sqlite_captures` and
`omitted_sqlite_sidecars`, both `#[serde(default)]` so v1 archives still read.
`backup create` prints `sqlite_captures: N` **always, including 0**, so an
operator can distinguish "this home had no databases" from "this build does not
capture them" — the second is what the defect looked like from outside.

## Gates

| gate | result |
|------|--------|
| `cargo test -p wcore-config --features sqlite --lib` | **571 passed; 0 failed; 0 ignored; 0 filtered out** (x2) |
| `cargo test -p wcore-config --features sqlite --lib sqlite_snapshot` | **4 passed; 0 failed; 0 ignored; 567 filtered out** |
| `cargo test -p wcore-cli --test backup_sqlite_capture` | **2 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo clippy -p wcore-config --features sqlite --all-targets` | no lints |
| `cargo clippy -p wcore-cli --all-targets` | no lints |
| `cargo fmt --all -- --check` | clean |
| harness self-test | 3 assertions, PASS |

**A near-vacuous gate I caught on myself:** `cargo test -p wcore-config` does NOT
enable the `sqlite` feature, so my four new unit tests never compiled. The totals
gave it away — 567 at base and 566+1 at my HEAD are the *same test set*. Reading
the count back is what caught it (section 3.2), and it is also why that first
run's single failure cannot be attributed to my change.

Also caught: `cargo test -p wcore-cli --test backup_sqlite_capture` against a
stale worktree listed the available targets instead of silently passing zero
tests — flavour (c) of the same class.

### Two failures in `cargo test -p wcore-cli`, both pre-existing

- `always_fails` — a string literal at `plugin/scaffold.rs:274`; the scaffolder
  emits `fn always_fails() { panic!("deliberate"); }` on purpose. At base it is
  not a `wcore-cli` test at all (`1874 filtered out`).
- `migrate_hermes::import_is_idempotent_without_overwrite` — **2 failures in 5
  runs at my HEAD**, matching the 3-in-11 the prior lane measured at base.
  Already open as F26-SC3-O2.

Neither was weakened, ignored or re-gated.

## Does restore now round-trip a live database?

**Yes, for the archive path, measured.** A home whose `memory.db` is being
written by three processes now restores to a database that passes
`integrity_check` and contains every row committed before the archive started —
6/6, including twice after merging integration.

## Still open — reported, not fixed

- **`restore --replace` captures the prior home into the journal's undo store as
  raw bytes.** If the home being replaced has a live writer, that undo store can
  hold the same inconsistent trio, so a rollback could restore a corrupt
  database. Same defect, other side of the operation. Not fixed here: it is a
  different code path (`journal.rs`) with its own interruption proofs, and
  changing it without repeating that proof would be worse than leaving it named.
  `wcore_config::sqlite_snapshot` is reusable for it as-is.
- **Only local ext4 was exercised.** The interaction between this capture and a
  network filesystem (where `sqlite_journal` selects TRUNCATE, so there is no
  WAL to fold) is untested. Expected benign; unmeasured.
- **`vec0` was not exercised end-to-end.** The chosen primitive is page-level so
  it cannot depend on the module, but I did not build a `vec0` fixture and prove
  it. The claim rests on the primitive's mechanism, not on a run.
- Windows: not exercised. The capture is platform-neutral Rust, but that is an
  expectation, not a measurement.

## Files

- `crates/wcore-config/src/sqlite_snapshot.rs` (new), `src/lib.rs` (module decl)
- `crates/wcore-cli/src/backup/archive.rs`, `mod.rs`, `remap.rs` (test fixture)
- `crates/wcore-cli/tests/backup_sqlite_capture.rs` (new)
- `crates/wcore-cli/Cargo.toml`, `crates/wcore-config/Cargo.toml`, `Cargo.toml`, `Cargo.lock`
- `scripts/sqlite-backup-{consistency-proof,writer,harness-selftest}.py`,
  `scripts/sqlite-snapshot-primitive-probe.py`
- evidence: `.planning/phases/26-migration-export-backup-restore/evidence/26-sc3-o1-sqlite/`

`crates/wcore-config/src/sqlite_journal.rs` was **not modified** — verified by
`git diff` against the merge-base. Neither shared-fence file
(`wcore-cli/src/lib.rs`, `main.rs`) was touched.
