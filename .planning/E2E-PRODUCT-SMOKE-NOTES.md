# NOTES — lane `e2e-product-smoke` (append-only, committed continuously)

**Lane** `lane/e2e-product-smoke` · **base** `75babf32` (verified `git rev-parse HEAD`)
**Worktree** `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-e2e-product-smoke`
**Mandate:** drive the real product cold-start, end to end, as a day-one user. Report every
step pass / fail / not-reached. Failures are findings, not stop conditions.

---

## Session log

### T+0 — worktree verified

```
/usr/bin/git rev-parse --show-toplevel
  -> /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-e2e-product-smoke
/usr/bin/git rev-parse --abbrev-ref HEAD -> lane/e2e-product-smoke
/usr/bin/git rev-parse HEAD -> 75babf329235484684ecee3a65973b0c197840c1
/usr/bin/git status --porcelain -> (empty)
```

Not `/Users/seandonahoe/dev/waylandcore`. Safe to proceed.

### T+5 — prior art read (so I do not re-measure what is already measured)

- `.planning/HEADLESS-KEYRING-FINDING.md` — HIGH, **already measured and fixed in a prior
  lane**: on a host with no OS keyring the product refused to start and the error's suggested
  remedy was wrong three ways (`[credentials]` key ignored; `[storage.credentials]` hard parse
  error; the actual working remedy `WAYLAND_VAULT_PASSPHRASE` named nowhere). My base is
  `75babf32` — **I must establish whether that fix is IN my base** before claiming step 1
  either way. If it is in, step 1 is a regression check; if it is not, I will hit the same
  wall and must say so as "already-known, present at my base", not as a new finding.
- `BL-23B-H1` (BACKLOG.md:1496) — session-journal read-back mismatch, MEDIUM, **non-reproducing**
  (92 runs / 153 tool events / 0 mismatches under load). Key fact for step 7: the *original*
  46 non-reproductions were produced by a harness pointed at `http://127.0.0.1:1` with a
  placeholder key, so **no run ever dispatched a tool event** — a dead instrument. The
  reach-proven harness is `scripts/f23-h1-repro-live.sh`, emitting `F23_H1_REACH=` per run.
  My step 7 must assert reach, not just "resume ok".
- `scripts/f24-secret-sweep.sh` — exists, has `--selftest`, refuses to report clean unless a
  known-positive control fired first. **Use it; do not hand-roll a grep.** The failure it was
  written against: zsh did not word-split `$PATHS`, grep errored, sweep reported "0 hits, clean".

### Instrument discipline for this lane

- All load-bearing numbers via `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/env cargo`, or
  `rtk proxy`. `rtk` rewrites `git log` (drops merges), `grep`, `cargo` (strips
  `0 ignored` / `0 filtered out`) and has returned `wc -c` = 0 for a 72-byte file.
- Every absence claim gets a known-positive in the same invocation.
- **Universal-denial green is a fail, not a pass.** Every sandbox refusal step is paired with
  a permitted command that must SUCCEED in the same run, from the same binary, same config.

### Planned journey (steps; will be graded pass / fail / not-reached)

| # | Step |
|---|---|
| 1a | Cold first run, no config, no keyring — what does the user see? |
| 1b | Cold first run, unhappy paths (bad config, missing dir, unwritable home) |
| 2 | Provider configured (FluxRouter, stdin-injected) + one real completed turn |
| 3a | Tools: Read / Write / Edit / Grep / Glob do real work |
| 3b | Bash through the sandbox: a PERMITTED command succeeds (paired control) |
| 3c | Bash through the sandbox: a DANGEROUS command is refused |
| 4 | A skill is invoked and demonstrably changes behaviour |
| 5 | Memory persists across a session boundary |
| 6 | MCP server connects and a tool is called through it |
| 7 | Session resume after restart (BL-23B-H1 surface; assert reach) |
| 8a | Clean exit — no orphan processes, no held leases |
| 8b | Crash exit (SIGKILL) — no orphan processes, no held leases |
| 9 | Secret sweep with liveness control; report hit count |

Open at this point: nothing measured yet. Next action is host + build reconnaissance
on `hetzner-dsm`.
