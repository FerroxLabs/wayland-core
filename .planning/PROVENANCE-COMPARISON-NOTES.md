# PROVENANCE COMPARISON — working notes (lane/provenance-comparison)

Started 2026-07-30. Base commit `57a41c7dcf2dcec63d1f631e9b8fbbefd01c6cfa`
(`revert(scrub): restore nine copyright attributions`).

Deliverable: `.planning/PROVENANCE-COMPARISON.md`. This lane changes NO source file.

## Instrument discipline in force

Per LANE-BRIEF §3b: every number that will appear in the report is produced by
redirecting an unproxied tool (`/usr/bin/git`, `/usr/bin/grep`) to a file and
reading the file with the Read tool. Nothing load-bearing is read from Bash stdout.

## Peer baseline assertion (done first — §3b-i)

| repo | pinned baseline | working-tree HEAD | pinned commit present in object store? |
|------|-----------------|-------------------|----------------------------------------|
| `resources/openclaw` | `11a0ad10` (2026-06-16, "test: make install-safe-path symlink tests compatible with Windows") | `3659c85e` (2026-07-18) | YES — `git cat-file -t` → `commit` |
| `resources/hermes-agent` | `dbe734be` (2026-06-27) | `d59b79fa` (2026-07-17) | YES — `git cat-file -t` → `commit` |

**The working trees are NOT at the pinned baselines** — both are ~1 month ahead.
Reading the checked-out files would compare against the wrong version. Mitigation:
all peer source is extracted with `git show <pinned-sha>:<path> > file` and read
from the file, never from the working tree. Every peer excerpt in the final report
is therefore at the pinned baseline, and I say so per-quote.

## Sites under examination (9)

1. `crates/wcore-providers/src/failover.rs:1` — FailoverReason taxonomy
2. `crates/wcore-providers/src/key_rotation.rs:1` — API key rotation pool
3. `crates/wcore-providers/src/classify.rs:1` — 3-tier failover classifier
4. `crates/wcore-providers/src/cache_observation.rs:1` — cache retention forensics
5. `crates/wcore-pricing/src/refresh.rs:1` — self-healing pricing layer
6. `crates/wcore-providers/src/retry.rs:738` — "Source: openclaw"
7. `crates/wcore-providers/src/anthropic.rs:307` — moving-breakpoint cache layout
8. `crates/wcore-channel-imessage/src/lib.rs:16` — via Wayland Desktop TS
9. `crates/wcore-channel-msteams/src/lib.rs:15` — via Wayland Desktop TS

## Control (required)

At least two `wcore-providers`/`wcore-pricing` modules with NO claimed peer
counterpart, run through the identical method, to calibrate what "independently
written Rust in this codebase" scores. Plus a deliberately-close pairing to prove
the method can FIND similarity (§3b-iii both-directions rule).

## Status log

- [x] worktree created, toplevel asserted
- [x] peer baselines asserted (both stale — mitigated by `git show` at pinned sha)
- [ ] our nine sites read
- [ ] peer counterpart search
- [ ] per-site comparison
- [ ] control modules
- [ ] Wayland Desktop TS chain check (sites 8, 9)
- [ ] report written
