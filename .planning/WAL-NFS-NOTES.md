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
