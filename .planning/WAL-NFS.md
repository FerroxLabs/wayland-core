---
lane: wal-nfs
finding: "RECON-GROK finding 1 — journal_mode=WAL set unconditionally, no network-filesystem detection"
sites-fixed: "4 of 5 briefed (5th is #[cfg(test)]); 0 further production sites found"
nfs-proof: "reproduced on a real loopback NFSv3 mount on hetzner-dsm; WAL corrupts, TRUNCATE does not"
pre-fix-behaviour: "NOT SIGBUS — silent database corruption. No process was ever killed by a signal."
new-finding: "the reported symptom is wrong; a 6th SQLite store exists but correctly sets no journal mode"
fence-exposure: "zero lines in crates/wcore-cli/src/{lib,main}.rs vs fab33493"
status: complete
---

# WAL on network filesystems — measured, fixed, proven

## Verdict

The defect is **real** and the fix is **proven on a real network filesystem**. But the
reported *symptom* is wrong, and the correction matters.

**The brief says the process dies with SIGBUS and that this "is not a corruption risk you can
catch". Measured, it is the exact opposite: the process survives and the database is
destroyed.** Across every run, both writers exited `rc=0` while `PRAGMA integrity_check`
failed and a writer could not see rows it had itself committed. That is worse than a crash,
not narrower: a SIGBUS is loud and immediate, this is silent, and one of the five databases
is `memory.db`, which holds long-term user memory.

## How it was proven

A real loopback NFSv3 export on `hetzner-dsm` (`/srv/walnfs-export` → `/mnt/walnfs`), mounted
a second time with `nosharecache` so the two clients have genuinely separate superblocks
(`dev=183` / `dev=184`) and therefore incoherent page caches — the single-host stand-in for a
network home mounted on two machines. Kernel-confirmed: `stat -f` reports `nfs` for the mount
and `ext2/ext3` for local, so the harness discriminates.

Two concurrent writers, 15s, **no fault injection**, journal mode read back from SQLite rather
than assumed:

| arm | write errors | writer's own rows visible | `integrity_check` |
|---|---|---|---|
| NFS + **WAL** | **11,826** | **0 of 5,102** | **corrupt** (`Rowid 2284 out of order`) |
| NFS + TRUNCATE | 0 | 5,886 of 5,886 | `ok` |
| NFS + DELETE | 0 | 2,673 of 2,673 | `ok` |
| local ext4 + WAL | 0 | 103,597 of 103,597 | `ok` |

The local-WAL arm is what proves WAL is worth keeping on local disks, and the TRUNCATE arm is
what proves the fix works rather than merely being different.

### The reproduction I threw away

My **first** SIGBUS reproduction was invalid and I caught it with its own control. Holding a
live WAL connection and truncating the `-shm` underneath it does raise SIGBUS on NFS — and
**it raised SIGBUS identically on ext4** (`rc=135`, signal 7, both arms). Truncating any
mmap'd file below a live mapping is fatal on every filesystem; that experiment measures POSIX
mmap semantics, not NFS. Had I run only the NFS arm I would have published a false
reproduction of the reported symptom. The known-negative arm is the only reason it was caught.

## The centralised selector

`crates/wcore-config/src/sqlite_journal.rs` — one decision point for the whole workspace.

- **Linux** `statfs(2)` `f_type`, masked to the low 32 bits so sign-extended magics on 32-bit
  kernels still match.
- **macOS** absence of `MNT_LOCAL` (authoritative remote signal, covers unknown types) plus an
  `f_fstypename` allowlist for FUSE bridges that claim `MNT_LOCAL` anyway.
- **Windows** UNC prefix plus `GetDriveTypeW` → `DRIVE_REMOTE`.
- Walks up to the nearest **existing** ancestor, so a not-yet-created directory does not
  defeat detection and needlessly demote a local disk.
- `WAYLAND_SQLITE_JOURNAL_MODE=wal|truncate` kill-switch, with typos warned rather than
  silently ignored.
- Leaving WAL is done under `locking_mode = EXCLUSIVE` so the conversion never maps the `-shm`
  it is trying to avoid.

**Deliberate divergence from the peer implementation** (`xai-sqlite-journal`, 779 lines, read
first): its classifier is one-sided and **fails open** — an unrecognised Linux magic returns
"local" and therefore WAL. Mine is two-sided (network list, local list, `Unknown`) and
`Unknown` takes rollback journaling. The asymmetry justifies it: guessing WAL wrong destroys
data silently, guessing TRUNCATE wrong costs read concurrency and is reversible by an env var.
A local allowlist (ext4, tmpfs, btrfs, xfs, zfs, **overlayfs**, f2fs, bcachefs, ntfs…) keeps
ordinary machines and CI on WAL, so this is not a blanket disable. Note the peer's own macOS
arm is already fail-safe in exactly this way — I made Linux consistent with it.

Cross-audited (codex gpt-5.6-sol): *"choose TRUNCATE for any unrecognized filesystem… WAL
safety is unproven; corruption is worse than reduced concurrency."* One auditor, not a panel.

**I did NOT adopt the peer's per-host database filenames** (`worktrees.db` →
`worktrees.h-<host>.db`). It defends against a concurrent pre-fix binary flipping a shared DB
back to WAL, and the peer justifies it on the grounds that "these DBs are all rebuildable
indexes/caches". That is false for us — `memory.db` is long-term user memory, and silently
splitting it per host would fragment a user's memory across machines to fix a narrower threat.

## Sites

Re-measured before building, with a live control (`rusqlite` → 40 files, non-zero).

| # | site | disposition |
|---|---|---|
| 1 | `wcore-repomap/src/store.rs:211` | fixed — mode injected (see below) |
| 2 | `wcore-memory/src/schema/mod.rs:45` | fixed — selects via `configure()` |
| 3 | `wcore-memory/src/schema/v1.sql:6` | **pragma removed entirely** |
| 4 | `wcore-swarm/src/audit.rs:196` | fixed — selects via `configure()` |
| 5 | `wcore-memory/src/db.rs:394` | **`#[cfg(test)]`** — deliberately forces WAL to assert 0600 perms on `-wal`/`-shm`. Left, annotated. |

`v1.sql` mattered more than it looks: it ran *after* the migration runner had chosen a mode and
would have silently reinstated WAL on network mounts.

### The repomap constraint

`wcore-repomap` is documented in AGENTS.md and in its own `Cargo.toml` as **deliberately
isolated with no internal `wcore-*` dependencies**. I initially wired it to `wcore-config`,
caught the violation, and reverted. Duplicating the probe there would breach "no duplicate code
across crates", so the mode is **injected**: `IndexStore::open` takes a `JournalMode` mirror
type and `wcore-cli` translates the detector's output into it — the same mirror-type pattern
the plugin API already uses to cross an isolation boundary (audit F2). The filesystem probe
still exists in exactly one place.

### A sixth SQLite store — correctly not a defect

`wcore-permissions/src/revocation.rs:66` opens a real on-disk database and was **not** in the
briefed list. It sets no `journal_mode` at all, so it uses SQLite's default rollback journal,
which needs no `-shm` and is NFS-safe. Reported, not changed.

### Prior art already in the tree, which does not help

`wcore-config/src/shell/executable_readiness.rs:540` already defines `is_network_path` — but it
returns `false` unconditionally off Windows and only matches UNC prefixes (its own test is
named `is_network_path_flags_unc_only`). It cannot see an NFS mount at `/home/user`. Two more
copies live in `wcore-tools`. The name was taken by a *syntactic* check; mine is a
*backing-filesystem* check and is deliberately named differently.

## Three-assertion self-test

`selector_discriminates_and_legacy_code_could_not`:

1. **known-positive** — ext4 magic `0xEF53` → `Local` → `Wal`.
2. **known-negative** — NFS magic `0x6969` → `Network` → asserted `!= Wal`.
3. **the old instrument would have missed it** — `legacy_unconditional()` returns `Wal`, is
   asserted *equal* to the local answer (so the local arm alone proves nothing) and *not equal*
   to the network answer. The brief notes this is trivially true here; it is true because the
   pre-fix value was a **string literal with no filesystem input at any of the sites**, which
   `git show fab33493` confirms. Encoding it executably keeps a future refactor honest.

## Live evidence (real binary, real mount)

```
LOCAL  (ext4) F23_INDEX=build ... records=5   journal_mode = wal      rows=5  integrity ok
NFS           F23_INDEX=build ... records=4   journal_mode = delete   rows=4  integrity ok
```

Both arms indexed real files — proof the database was genuinely opened and written, not merely
"not crashed".

**Instrument check:** NFS does not simply refuse WAL. The same binary, same mount, with
`WAYLAND_SQLITE_JOURNAL_MODE=wal` produced `journal_mode = wal` and 4 rows. The difference is
the selector alone. Per §3b-ii the forced arm was confirmed from the product's own log line
(`... mode="WAL" source="env"`), not from the environment I exported.

## What I did NOT establish

- **The product-level corruption test did not reproduce.** Six rounds of two concurrent
  `wayland-core index build` processes across the two incoherent NFS clients, WAL forced, left
  `integrity_check = ok` in both arms. `index build` is short-lived and the processes barely
  overlap, so the 15s sustained-write window the SQLite-level hammer needed never opens. The
  mechanism and the mode selection are proven; the product corrupting a database is **not**
  something I observed.
- Windows and macOS classifier arms are unit-tested against their pure classifiers but were
  **not** exercised on a real SMB/AFP mount.
- Only one cross-auditor responded within budget; this was not a four-way panel.

## Gates

| gate | result |
|---|---|
| `cargo test -p wcore-config --lib sqlite_journal` | **11 passed; 0 failed; 0 ignored; 551 filtered out** |
| `cargo test -p wcore-memory` | 348 + 20 suites, **0 failed** |
| `cargo test -p wcore-repomap` | 38/13/5/2, **0 failed** |
| `cargo test -p wcore-swarm` | 114 + 10 suites, **0 failed** |
| `cargo clippy` (5 touched crates, `-D warnings`) | clean |
| `cargo fmt --all` | diff-free |
| fence vs `fab33493` | **zero lines** |

Counts read from unproxied `/usr/bin/env cargo`; `0 ignored` / `filtered out` fields are
present, so the suites genuinely executed rather than exiting 0 on zero tests.

## Follow-ups (non-blocking)

- `wcore-tools` carries two more UNC-only `is_network_path` copies (`vision_tools.rs:167`,
  `media_intake.rs:185`). Out of scope here; candidates for consolidation onto `classify_path`.
- A long-lived-connection product test (a real session driving `wcore-memory`) would close the
  gap left by M9.
