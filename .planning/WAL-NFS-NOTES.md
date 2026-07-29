# WAL-NFS lane — running notes

Base: `fab33493`. Branch `lane/wal-nfs`. Append after every measurement (LANE-BRIEF §6b-i).

---

## M1 — re-measured the five sites (2026-07-29)

Instrument liveness (known-positive in the same sweep): `rusqlite` → **40 files**. Non-zero,
so the instrument discriminates.

```
/usr/bin/grep -rni "journal_mode" crates/
```

6 hit lines / **5 distinct sites**, all unconditional WAL, none carrying detection:

| # | Site | Form |
|---|------|------|
| 1 | `wcore-repomap/src/store.rs:211` | `query_row("PRAGMA journal_mode = WAL")` |
| 2 | `wcore-memory/src/db.rs:394` | `"PRAGMA journal_mode=WAL; ..."` (execute_batch) |
| 3 | `wcore-memory/src/schema/v1.sql:6` | `PRAGMA journal_mode = WAL;` |
| 4 | `wcore-memory/src/schema/mod.rs:45` | `pragma_update(None,"journal_mode","WAL")` |
| 5 | `wcore-swarm/src/audit.rs:196` | `pragma_update(None,"journal_mode","WAL")` |

(audit.rs 196+197 are one statement over two lines — 6 lines, 5 sites. Confirmed.)

Whole-repo sweep outside `crates/`: **zero** further hits.

## M2 — INSTRUMENT DEFECT CAUGHT MID-LANE (the §3b-i trap, live)

My first concept-sweep used **unquoted** `--include=*.rs`. zsh ate the glob:

```
(eval):1: no matches found: --include=*.rs
```

…and the piped `grep -c .` dutifully printed **`0`**. That `0` was about to be read as
"no network-FS detection exists anywhere" — a *correct conclusion reached by a dead
instrument*. Re-run with quoted globs returned **283** `sqlite` hits and a real answer.
Logged because the brief predicted exactly this shape and it still nearly landed.

## M3 — a sixth SQLite DB exists; it is NOT a sixth WAL site

`crates/wcore-permissions/src/revocation.rs:66` — `rusqlite::Connection::open(...)`,
a real on-disk store not in the briefed list. It sets **no** `journal_mode`, so it runs
SQLite's default rollback journal, which needs no `-shm` mapping and is NFS-safe.
So: a site the brief missed, but correctly not in the defect set. Reported, not changed.

## M4 — `is_network_path` ALREADY EXISTS in wcore-config, and does not help

`crates/wcore-config/src/shell/executable_readiness.rs:540`

```rust
fn is_network_path(path: &Path, platform: ExecutablePlatform) -> bool {
    if platform != ExecutablePlatform::Windows { return false; }   // <-- unconditional false off-Windows
    path.to_str().is_some_and(|p| p.starts_with(r"\\") || p.starts_with("//"))
}
```

Its own test is named `is_network_path_flags_unc_only`. It detects **UNC prefixes only**,
and returns `false` for every non-Windows path. An NFS mount at `/home/sean` is a plain
POSIX path — this returns `false` for it. Two more copies in `wcore-tools`
(`vision_tools.rs:167`, `media_intake.rs:185`), same UNC-only shape.

**Consequence for design:** the name is taken by a *syntactic* check. My selector needs a
*backing-filesystem* check (statfs) and must not be confused with it. Distinct name.

## Still to establish

- [ ] Read grok-build `xai-sqlite-journal` (read-only) before designing.
- [ ] Does the pre-fix code actually SIGBUS on a real NFS mount? Loopback NFS on hetzner.
- [ ] Centralised selector in `wcore-config`; 5 call sites converted.
- [ ] Three-assertion self-test incl. "pre-fix would have chosen WAL in both".

---

## M5 — real loopback NFS mount stood up on hetzner (2026-07-29)

`nfs-kernel-server` installed; export `/srv/walnfs-export` → mount `/mnt/walnfs`, NFSv3 over TCP
to 127.0.0.1. Kernel-confirmed discriminating control:

```
/mnt/walnfs  fstype=nfs          <- statfs magic 0x6969
/root        fstype=ext2/ext3    <- local
```

**WAL is accepted on NFS.** `PRAGMA journal_mode=WAL` returns `wal` (read back, not assumed),
table created, row inserted, `rows=1`. A single sequential client does **not** crash.
So the naive claim "WAL on NFS crashes" is FALSE as stated.

## M6 — MY FIRST SIGBUS REPRODUCTION WAS A DEAD INSTRUMENT. The control caught it.

Probe: hold a live WAL connection (shm mapped), have a second actor `truncate -s 0` the
`-shm` (modelling "a peer host rebuilds the shm during recovery"), then touch the mapping.

```
### NFS   (fstype=nfs)         TRUNCATED -shm  -> rc=135  KILLED BY SIGNAL 7 (BUS)
### LOCAL (fstype=ext2/ext3)   TRUNCATED -shm  -> rc=135  KILLED BY SIGNAL 7 (BUS)
```

**The local arm SIGBUSed identically.** Truncating any mmap'd file below a live mapping raises
SIGBUS on every filesystem — that is POSIX mmap semantics, not an NFS defect. So the experiment
does not discriminate, and a SIGBUS from it is worth **nothing** as evidence about NFS.

Had I run only the NFS arm I would have published a false reproduction. The known-negative arm
is the entire reason this was caught. Logged as the lane's own instance of the class.

Truncation is therefore an INVALID model: it conflates "peer host rebuilds the shm" with
"somebody truncated the file", and the latter is fatal everywhere.

### What actually needs proving
On ONE host all processes share a page cache, so the `-shm` mapping IS coherent and WAL works.
The documented failure needs a genuinely second client. Next: mount the same export twice with
`nosharecache` (separate superblocks → separate inode/page caches → genuinely incoherent mmap)
and run two concurrent WAL writers with NO artificial truncation.

## M7 — VALID reproduction. The defect is real, and it is NOT the reported one.

Two genuinely incoherent NFS clients of one export (`nosharecache` → separate superblocks,
`dev=183` vs `dev=184`), same backing file, **no artificial truncation** — just two processes
using SQLite concurrently. 15s each, journal mode read back from SQLite, not assumed.

```
NFS + WAL        A writes_ok=5102 errs=3757 rows_by_me_visible=0    lasterr="file is not a database"
                 B writes_ok=2668 errs=8069 rows_by_me_visible=2214 lasterr="no such table: t"
                 integrity_check -> *** in database main ***  Rowid 2284 out of order (+more)
NFS + TRUNCATE   A writes_ok=5886 errs=0 rows_by_me_visible=5886    integrity_check -> ok
NFS + DELETE     A writes_ok=2673 errs=0 rows_by_me_visible=2673    integrity_check -> ok
LOCAL + WAL      A writes_ok=103597 errs=0 / B 100473 errs=1(locked) integrity_check -> ok
```

**Three conclusions, one of which contradicts the brief:**

1. **The defect reproduces.** WAL on a real network filesystem destroys the database.
   11,826 failed writes and a failed `integrity_check` against 0 failures on the identical
   mount in TRUNCATE.
2. **It is NOT SIGBUS. It is silent corruption.** Neither arm was ever killed by a signal —
   both exited `rc=0` every run. The brief's "the process is killed / it is not a corruption
   risk you can catch" is **backwards**: the process lives and the *data* dies. Arm A
   committed 5102 rows and could then see **zero** of them.
   That is worse, not narrower — a crash is loud, this is silent, and `memory.db` holds
   long-term user memory.
3. **The fix is proven, not assumed.** TRUNCATE and DELETE both survive the identical
   workload on the identical mount with 0 errors and `integrity_check = ok`.

Local-disk WAL control passed in the same session (204k writes, `ok`), so the harness
discriminates and WAL-on-local is genuinely worth keeping.

## M8 — the fix, and its live proof through the real binary

Centralised in `wcore-config::sqlite_journal` (statfs on Linux, MNT_LOCAL+fstype on macOS,
GetDriveTypeW on Windows). Four production sites converted; the fifth briefed site is
`#[cfg(test)]`.

Live, via the real `wayland-core index build`, on hetzner:

```
LOCAL  (ext4) F23_INDEX=build ... records=5   persisted journal_mode = wal      rows=5 integrity ok
NFS           F23_INDEX=build ... records=4   persisted journal_mode = delete   rows=4 integrity ok
```

**Artifact ruled out.** NFS does not simply refuse WAL: the same binary on the same mount with
`WAYLAND_SQLITE_JOURNAL_MODE=wal` produced `journal_mode = wal`, 4 rows. The difference is the
selector and nothing else.

§3b-ii compliance: the forced arm was confirmed from the product's OWN log line
(`sqlite journal mode forced by WAYLAND_SQLITE_JOURNAL_MODE ... mode="WAL" source="env"`),
not from the env I exported.

## M9 — HONEST NEGATIVE: the product-level corruption test did not reproduce

Six rounds of two concurrent `wayland-core index build` processes through the two incoherent
NFS clients, WAL forced:

```
PREFIX  (WAL forced)  journal_mode=wal     integrity=ok  rows=5
POSTFIX (detection)   journal_mode=delete  integrity=ok  rows=4  (1 benign UNIQUE-constraint race)
```

**No corruption in either arm.** `index build` is short-lived and the two processes barely
overlap, so the window the SQLite-level hammer exploited (15s of sustained concurrent writes)
never opens. So I can claim the mechanism and the mode selection, but I did NOT observe the
product itself corrupting a database. Stated as a limit on the evidence, not glossed.

The exposure is still real for the long-lived connections: `wcore-memory` holds a connection
for a whole session and `wcore-swarm`'s audit trail is appended across a run.

## M10 — pre-fix evidence, stated precisely

Against merge-base `fab33493`, the pre-fix selection at all sites was a string literal with no
filesystem input of any kind:

```
store.rs:211  .query_row("PRAGMA journal_mode = WAL", ...)
audit.rs:196  conn.pragma_update(None, "journal_mode", "WAL")
schema:45     conn.pragma_update(None, "journal_mode", "WAL")?;
```

There is no branch, so "the pre-fix code would have chosen WAL on both filesystems" is not an
inference — the value is a constant. Encoded executably as
`SqliteJournalMode::legacy_unconditional()` and asserted in the self-test.

## M11 — fence + cross-audit

Fence exposure vs `fab33493`: `crates/wcore-cli/src/{lib,main}.rs` — **zero lines**, empty diff.
Control on the same base shows 14 files / 865 insertions, so the diff instrument is live.

Cross-audit (codex gpt-5.6-sol, last block) on the one real judgement call — unknown filesystem
→ WAL or TRUNCATE: **"choose TRUNCATE for any unrecognized filesystem"**, agreeing with the
implementation and against the reference implementation's fail-open default. One auditor, not
a full panel; the decision is anyway backed by the measured asymmetry.
