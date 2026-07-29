# F26-SC3-O1 — NOTES (lane/backup-sqlite)

Append-only working log. Committed early and re-committed after each measurement
per LANE-BRIEF section 6b-i.

## Position

- Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-backup-sqlite`,
  branch `lane/backup-sqlite`, merge-base `eaff921d`.
- `git rev-parse --show-toplevel` verified = the lane path (NOT `dev/waylandcore`).
- hetzner worktrees: `/root/wayland-backup-sqlite` (fix) and
  `/root/wayland-backup-sqlite-base` (pre-fix binary, for repeat known-negatives).

## Measurement 1 — the defect is real in the source (instrument proven alive)

```
/usr/bin/grep -rc 'payload' crates/wcore-cli/src/backup/                       # POSITIVE CONTROL
/usr/bin/grep -rniE 'sqlite|rusqlite|\.db\b|wal|shm|journal_mode|checkpoint|vacuum' \
    crates/wcore-cli/src/backup/
```

Positive control (instrument alive): 58 hits in `archive.rs`, 22 in `mod.rs`,
19 in `restore.rs`, 13 in `journal.rs`. Concept sweep for SQLite: every hit is
either "journal" meaning *this module's* undo journal, or `memory.db` written by
`std::fs::write` as a plain-text fixture. Zero `rusqlite`, `PRAGMA`,
`wal_checkpoint`, `VACUUM`.

## Measurement 2 — the mechanism

`archive.rs:157` — `let bytes = std::fs::read(abs)` inside a loop over every
payload in sorted order. Three files, three independent reads, three instants.

## Measurement 3 — the torn read, through the real product binary

`scripts/sqlite-backup-consistency-proof.py`, three stock-Python `sqlite3`
writers, 158 MiB `memory.db`, on `hetzner-dsm`.

| run | binary | arm | writers alive across archive | restored sidecars | integrity_check | missing committed rows | verdict |
|-----|--------|-----|------------------------------|-------------------|-----------------|------------------------|---------|
| run1 | base | concurrent | +23,131 / +23,654 / +23,776 | shm + wal | **corrupt** (100 lines) | 0 | FAIL |
| b1 | base | concurrent | +24,260 / +24,431 / +23,833 | shm + wal | **corrupt** (101 lines) | 0 | FAIL |
| b2 | base | concurrent | +21,558 / +21,885 / +21,853 | shm + wal | **corrupt** (101 lines) | **20** | FAIL |
| b3 | base | concurrent | +23,356 / +23,085 / +22,895 | shm + wal | **corrupt** (101 lines) | 0 | FAIL |
| run2 | base | **sequenced** | writers exited first | none | `ok` | 0 | **PASS** |
| run3–6 | fixed | concurrent | ~+23,000 each | none | `ok` | 0 | PASS x4 |
| m1 | merged | concurrent | +20,403 / +19,536 / +19,921 | none | `ok` | 0 | PASS |
| m2 | merged | sequenced | writers exited first | none | `ok` | 0 | PASS |

`backup create` and `backup restore` both exited **0** in every FAIL row.

## Instrument defect found and REPAIRED IN-LANE (section 6b-ii)

Run 1 aborted with `ABORTED-NO-CONCURRENCY-DURING-ARCHIVE` reporting `w1: 2820
-> 0`. The guard was right to fire; the fault was mine. The writer published its
committed high-water mark with `open(path,"w")`, which truncates before writing,
and the driver read that zero-byte window and coerced it to `0`.

Repaired, not documented-and-left: `os.replace` publish, and `read_progress`
RAISES on an unreadable marker instead of substituting a number.
`sqlite-backup-harness-selftest.py` holds the three assertions, the third being
the load-bearing one — the OLD publisher produces **28,777 observable partial
reads** on hetzner and the repaired one **0**.

## Design decision

Panel (LANE-BRIEF section 4) split 2-1: codex=A, gemini=A, kimi=B — and split on
FACTS, not preference, so the tie went to measurement.

- Both A-voters and the B-voter agreed a `BEGIN IMMEDIATE` + byte-copy is NOT
  safe, because a PASSIVE checkpoint does not take the writer lock. Kimi's
  proposed repair for B (`wal_checkpoint(TRUNCATE)` from a second connection
  while the first holds `BEGIN IMMEDIATE`) **self-deadlocks**: TRUNCATE needs
  the very writer lock the first connection is holding. B rejected.
- Within A's architecture (snapshot through SQLite rather than through the
  filesystem), chose the **online backup API** over `VACUUM INTO`, because the
  backup API copies PAGES and never executes the schema. VACUUM INTO would
  depend on `vec0` being registered and on a documented "may change ROWIDs".
- **Honest negative:** the ROWID renumbering was probed
  (`sqlite-snapshot-primitive-probe.py`) and **did NOT reproduce** — VACUUM INTO
  preserved rowids and the external-content FTS5 mapping at SQLite 3.53.2. It is
  rejected for depending on a documented "may", not for an observed failure.
  Recorded because rejecting an option on a hazard I could not demonstrate would
  otherwise read as evidence I do not have.

## Verification

- `cargo test -p wcore-config --features sqlite --lib` — **571 passed; 0 failed;
  0 ignored; 0 filtered out**, twice. First run showed 1 unrelated failure
  (`resolves_same_and_cross_provider_fallbacks_with_independent_credentials`);
  it passes in isolation and the test SET is identical to base (567), so my
  feature-gated module contributed 0 tests to that run and cannot be the cause.
- `cargo test -p wcore-config --features sqlite --lib sqlite_snapshot` —
  **4 passed; 0 failed; 0 ignored; 567 filtered out**. Run explicitly, because
  the plain `-p wcore-config` invocation does NOT enable the feature and
  therefore never compiled them.
- `cargo test -p wcore-cli --test backup_sqlite_capture` — **2 passed; 0 failed;
  0 ignored; 0 filtered out**.
- `cargo clippy -p wcore-config --features sqlite --all-targets` and
  `-p wcore-cli --all-targets` — no lints.
- `cargo fmt --all -- --check` — clean.

### Two pre-existing failures, verified as such

- `always_fails` — a string literal in `plugin/scaffold.rs:274`; the scaffolder
  emits `fn always_fails() { panic!("deliberate"); }` by design. Filtered out
  entirely at base (`1874 filtered out`), i.e. not a test of `wcore-cli` at all.
- `migrate_hermes::import_is_idempotent_without_overwrite` — **2 failures in 5
  runs at my HEAD**, matching the 3-in-11 rate the prior lane measured at base
  (F26-SC3-O2, HashMap section ordering).
