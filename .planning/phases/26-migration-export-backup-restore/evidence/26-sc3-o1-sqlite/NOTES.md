# F26-SC3-O1 — NOTES (lane/backup-sqlite)

Live, append-only. Committed early per LANE-BRIEF §6b-i.

## Position

- Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-backup-sqlite`,
  branch `lane/backup-sqlite`, base `eaff921d`.
- `git rev-parse --show-toplevel` verified = the lane path (NOT `dev/waylandcore`).

## Measurement 1 — the defect is real in the source (instrument proven alive)

Command (unproxied `/usr/bin/grep`, run from the worktree root):

```
/usr/bin/grep -rc 'payload' crates/wcore-cli/src/backup/                       # POSITIVE CONTROL
/usr/bin/grep -rniE 'sqlite|rusqlite|\.db\b|wal|shm|journal_mode|checkpoint|vacuum|memory\.db' \
    crates/wcore-cli/src/backup/
```

Positive control (instrument alive): `journal.rs:13 platform_paths.rs:29 remap.rs:3
restore.rs:19 mod.rs:22 archive.rs:58` — 58 hits in archive.rs alone.

Concept sweep for SQLite: **every** hit is either the word "journal" meaning *this
module's* undo journal, or `memory.db` written by `std::fs::write` as a plain text
fixture in `journal.rs` tests. Zero `rusqlite`, zero `PRAGMA`, zero `wal_checkpoint`,
zero `VACUUM`. Concept vocabulary searched, not one keyword (LANE-BRIEF §3b-i.3).

## Measurement 2 — the mechanism

`crates/wcore-cli/src/backup/archive.rs:157-166`:

```rust
for Payload { rel, abs } in &payloads {
    let bytes = std::fs::read(abs)...;
```

Each payload is an **independent `std::fs::read` at a different instant**. `mod.rs`
`collect_payloads`/`walk` carries every regular file, with no notion of a database
and its sidecars being one object. So `memory.db`, `memory.db-wal` and `memory.db-shm`
are three unrelated reads. Confirms the `26-SC3-ROLLBACK-SUMMARY.md` finding.

## Still to establish

- [ ] Torn read on CURRENT code, with every writer proven to have reached a START
      marker (LANE-BRIEF §6a-i).
- [ ] Design decision: `VACUUM INTO` / `sqlite3_backup` vs quiesce+checkpoint.
- [ ] Fixed-arm proof: `integrity_check=ok` AND every committed row present.
