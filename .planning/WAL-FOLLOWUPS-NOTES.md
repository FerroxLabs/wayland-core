# WAL follow-ups lane — running notes

Base: `5ce245be` (`lane/wal-nfs` HEAD). Branch `lane/wal-followups`.
Append after every measurement (LANE-BRIEF §6b-i). Do not batch to the end.

Spec I am closing (NOT re-deriving): `.planning/WAL-NFS.md` §"What I did NOT establish".

| Gap | Claim to close | Status |
|---|---|---|
| 1 | Product-level corruption: real product, long enough + concurrent enough, WAL on network mount | OPEN |
| 2 | Windows `GetDriveTypeW` + macOS `MNT_LOCAL` arms live on a real network mount, both sides | OPEN |
| 3 | Three divergent `is_network_path` copies routed through the centralised selector | OPEN |

Prior lane's established facts I am taking as given (measured, not re-run):
- WAL on a genuinely-incoherent NFS mount corrupts: 11,826 write errors, 0/5,102 rows visible,
  `integrity_check` corrupt, `rc=0` throughout. Raw-SQLite level.
- The selector at `crates/wcore-config/src/sqlite_journal.rs` picks correctly live on Linux.
- `index build` is too short-lived to open the window (six rounds, `integrity_check=ok`).
- `wcore-repomap` must NOT gain a `wcore-*` dep — mode is injected via a mirror type from
  `wcore-cli`. Do not re-break that boundary.

---

## M0 — lane opened

Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-wal-followups`,
`git rev-parse --show-toplevel` confirmed (NOT `/Users/seandonahoe/dev/waylandcore`).
Branch `lane/wal-followups` @ `5ce245be`.

### Gap 1 — the shape the driver must have

The prior lane's raw-SQLite hammer needed **15s of sustained concurrent writes** to corrupt.
`index build` opens, writes ~5 rows, closes. So the driver must exercise a product path that
holds a connection and writes repeatedly. Per WAL-NFS.md the two exposed long-lived paths are:

- `wcore-memory` — holds a connection for a whole session (and `memory.db` is user memory);
- `wcore-swarm` audit trail — appended across a run.

Plan: drive the **product's own public API / shipped binary**, not `rusqlite`, from two
processes across the two incoherent NFS clients, WAL forced, for long enough to beat 15s by a
wide margin. If it does not corrupt, that is the reportable result — see brief: "do not
manufacture a reproduction". Record duration, concurrency, and write volume either way.

### Known-negative discipline for this lane

Every absence I report gets a same-invocation known-positive (§3b-i). Every "did not corrupt"
gets a paired arm that DOES corrupt on the same instrument, or it is not a measurement.
