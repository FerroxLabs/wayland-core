# NOTES — lane/windows-legs-sweep

Started 2026-07-30. Base `b2ddf113`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-windows-legs-sweep`.

## Premise checks (LANE-BRIEF: "your brief's measurements are probably stale")

| Claim | Status | Evidence |
|---|---|---|
| `ssh SeanD@seandesktop` works | **HELD** | `hostname` → `SeanDesktop`, `OK`, rc=0, BatchMode=yes |
| flock-based leases exist | **HELD** | 7 files under `crates/` match `flock` (quoted glob, `/usr/bin/grep`) |

flock files at base:
```
crates/wcore-eval-scenarios/src/process_tree.rs
crates/wcore-agent/src/channel_lease.rs
crates/wcore-agent/src/session_journal/lease.rs
crates/wcore-swarm/src/worktree_cleanup.rs
crates/wcore-swarm/src/worktree_tests.rs
crates/wcore-gateway/src/pidlock.rs
crates/wcore-cron/src/lease.rs
```
Known-positive control for the grep instrument: `grep -c "fn " crates/wcore-agent/src/engine.rs`
→ **924** (non-zero ⇒ instrument alive).

## Legs, in the order assigned

1. reload lease fix — Windows run. Windows locking is **mandatory**, not advisory.
2. `F21-04-03` Windows re-proof — journal-head CAS, `session_journal/reducer.rs:708`.
3. Phase 22 M1–M5 on Windows.
4. `21-C3` Windows.

## Log

- (t0) worktree created, SHA asserted `b2ddf113`, branch `lane/windows-legs-sweep`.
- (t0) Windows reachable.
