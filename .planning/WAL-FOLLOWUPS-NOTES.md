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

---

## M1 — the zsh glob trap fired again, first grep of the lane

`/usr/bin/grep -rn "is_network_path" crates/ --include=*.rs` → `(eval):1: no matches found`,
and the piped `wc -l` printed **0**. Unquoted globs, exactly as §3b-i predicts. Quoted
(`"--include=*.rs"`) with a same-invocation known-positive (`rusqlite` → **34 files**, non-zero)
the same sweep returned **13 hit lines**. Second lane running to hit this; it is not rare.

## M2 — Gap 3 is wider than briefed: FIVE implementations, not three

Same sweep, plus a follow-on for the concept rather than the keyword (§3b-i rule 3):

| # | site | form | `\\?\C:\` (verbatim LOCAL) | `//srv/sh` on Unix |
|---|---|---|---|---|
| 1 | `wcore-config/shell/executable_readiness.rs:540` `is_network_path` | string prefix, hard `false` off-Windows | **true (wrong)** | false |
| 2 | `wcore-tools/vision_tools.rs:167` `is_network_path` | `Component::Prefix` UNC | false (right) | false |
| 3 | `wcore-tools/media_intake.rs:185` `is_network_path` | string prefix | **true (wrong)** | true |
| 4 | `wcore-config/sqlite_journal.rs:349` `is_windows_unc` | string, excludes `\\?\`/`\\.\` | false (right) | false |
| 5 | `wcore-tools/path_validation.rs:172` `looks_like_unc` + `:216` `looks_like_device_or_verbatim` | separator-normalising, platform-authoritative on Windows | false (right, and reported as a *distinct* error) | true |

So the brief's "three divergent copies" undercounts: there are **five**, they disagree on two
concrete inputs, and #5 is a materially better implementation than the other four that the
other four do not use.

**And #2 and #3 are redundant.** Both `vision_tools::load_local_image` and
`media_intake::admit_path` call their local `is_network_path` and then call
`validate_user_path`, which already runs #5. The local copies change only which error is
returned. To be measured, not assumed.

## M3 — GAP 1 CLOSED: the product corrupts `memory.db` on a network mount

Driver: `crates/wcore-memory/examples/nfs_memory_hammer.rs`. Uses `Memory::open()` — the
constructor production bootstrap calls — and writes via `record_episode` /
`update_user_model`, the calls the memory tool makes. Not `rusqlite` in a loop.

Two writers, one per incoherent NFS client (`dev=183` / `dev=184`, `nosharecache`), same
backing file, 4 tokio tasks each, 150s, WAL forced. Journal mode read back **from the
database**, not from the environment (§3b-ii).

```
A  journal_mode=wal writes_ok=13179 writes_err=12409
   own_episodes_visible=<query failed: database disk image is malformed>
   integrity_check=*** in database main ***
B  journal_mode=wal writes_ok=6476  writes_err=24712
   own_episodes_expected=3238  own_episodes_visible=3834   <- sees MORE rows than it wrote
   integrity_check=*** in database main ***
```

**37,121 write errors. Both processes exited normally — no signal.** The prior lane's
correction holds at the product level too: the process lives, the user's long-term memory
database dies. Writer A could not even count its own rows: the query itself failed with
`database disk image is malformed`.

### Why the prior lane's attempt did not reproduce, and mine did

Not duration alone. **The first run of this driver also failed to reproduce — for a different
reason, and it looked like a pass.** Writer A died at startup with
`FATAL open_failed err=memory DB: database is locked`: the schema migration takes a long
exclusive lock, and on NFS the second opener loses it. Only B ran, so the arm measured a
*single* writer and reported `integrity_check=ok` with 45,342 clean writes. A one-sided run
that reads as a clean pass. Pre-creating the database so neither writer loses the race at open
is what made the two processes actually overlap.

So `index build` not reproducing is only half the story: **a concurrency test on this path can
also fail to reproduce because one participant never started.** Assert both writers reached
`START`, not just that the run exited.

## M4 — the two controls that make M3 a measurement rather than an anecdote

Identical driver, identical concurrency, identical duration. Only the journal mode and the
filesystem change.

| arm | filesystem | mode (read back from the DB) | writes ok | writes err | own rows visible | `integrity_check` |
|---|---|---|---|---|---|---|
| 1 defect | NFS ×2 incoherent | `wal` (forced) | 19,655 | **37,121** | A: *query itself failed* / B: 3,834 vs 3,238 | **corrupt** |
| 2 fix | NFS ×2 incoherent | `truncate` (**selector**) | 47,170 | **0** | 11,507/11,507 · 12,078/12,078 | `ok` |
| 3 control | local ext4 | `wal` (forced) | 285,104 | **0** | 76,012/76,012 · 66,540/66,540 | `ok` |

Arm 2 is the fix, at the product level, on the same mount that destroyed the database in arm 1.
Arm 3 is what stops arm 1 from being explained by "the driver is simply racy" — the same code,
the same four-task concurrency, WAL, 285k writes, zero errors. **Gap 1 CLOSED.**

Arm 3 also re-earns WAL on local disks: 285k writes vs 47k, so the selector is worth having
rather than a blanket disable.

## M5 — GAP 2 CLOSED: macOS and Windows exercised on real network mounts

New test `crates/wcore-config/tests/live_fs_class.rs`. `#[ignore]`d, and when the env is
absent it **panics** rather than returning — an env-gated early `return` is the measured
"printed `5 passed` for zero work" defect and this cannot do it.

**macOS** — this Mac (`Darwin arm64`). Loopback-free SMB: Samba on hetzner bound to `127.0.0.1`
only, reached through an ssh tunnel, mounted with `mount_smbfs` as an ordinary user (no sudo,
nothing on hetzner exposed publicly). Write-through verified: a file written on the Mac was read
back out of `/srv/walsmb` on hetzner. The mount shows **no `local` flag** — i.e. no `MNT_LOCAL`,
which is the exact signal the classifier keys on.

```
LIVEFS os=macos network=/Users/seandonahoe/walfu-smb -> Network / Truncate
LIVEFS os=macos local=/private/tmp                   -> Local / Wal
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Also ran `sqlite_journal::tests::mnt_local_matches_libc` — `#[cfg(target_os = "macos")]`, so it
had never executed on any host in this program. **1 passed.** `MNT_LOCAL` really is `0x1000`.

Both runs used the LANE-BRIEF §0 Darwin exception (single crate, single test file). Disclosed.

**Windows** — `SeanD@seandesktop`, all work under `D:\walfu` (never `C:\`). `net use Z: \\localhost\D$`
maps a real drive through the SMB redirector. `fsutil` is the independent control:
`Z: - Remote/Network Drive`, `D: - Fixed Drive`.

**`Z:\walfu\probe` and `D:\walfu\probe` are the same directory**, so nothing differs between the
two arms except the access path — the answer is attributable to `GetDriveTypeW` alone. This is
the arm no string inspection can reach: `Z:\` is spelled exactly like a local drive letter.

```
LIVEFS os=windows network=Z:\walfu\probe -> Network / Truncate
LIVEFS os=windows local=D:\walfu\probe   -> Local / Wal
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Both hosts, both failure controls fire** (§3.2 — a gate I cannot make fail is not a gate):
- local path handed in as the network path → `FAILED. 0 passed; 1 failed`
- no env at all → `FAILED`, with the "must FAIL rather than silently pass" message.

Note two `cargo` builds were running on the Windows box concurrently (the self-hosted runners);
this test is not timing-sensitive, so contention does not affect the result.
