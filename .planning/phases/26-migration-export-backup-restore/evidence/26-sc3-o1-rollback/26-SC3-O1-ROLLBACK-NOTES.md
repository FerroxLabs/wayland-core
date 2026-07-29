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

## Status

Nothing built or measured yet beyond the above reading. Next: hetzner worktree,
reproduce at base under load.
