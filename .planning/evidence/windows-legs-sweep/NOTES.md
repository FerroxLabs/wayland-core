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

## Windows workspace

`D:\wls\repo` (clone @ lane HEAD), `D:\wls\repo-cron` (second clone, negative controls),
targets `D:\wls\target`, `D:\wls\target-cron`, `D:\wls\target-cron-neg`, outputs `D:\wls\out`.
Nothing at the root of `C:\`. `C:\actions-runner-*` untouched.
Toolchain on SeanDesktop: cargo 1.95.0, rustc 1.95.0, git 2.54.0.windows.1.

## Measured premise corrections

- **`session_journal/reducer.rs:708` is STALE.** The CAS the finding cites now lives at
  **`reducer.rs:1570`**. Verified with `/usr/bin/grep -rn "prior cursor does not match"`,
  1 hit. The file grew; line 708 is now unrelated goal-task handoff code.
- The repair is `journal.append_built_from_head` (`session_journal.rs:541`), 4 call sites.
  It is a **pure `Mutex` scoping change** — nothing platform-specific. Windows vs Linux
  differs only in *scheduling*, which is exactly why Windows hit it 6/6 and Linux 3/8.
- `crates/wcore-agent/src/channel_lease.rs` has **zero** `cfg(unix)`/`cfg(windows)` gates
  (control: `session_journal/lease.rs` has 17). The channel poll lease delegates entirely
  to `wcore_cron::lease::ScheduleLease::attempt_named`, so proving that primitive on
  Windows proves the channel lease's exclusion too.

## Results so far (all counts read from files with the Read tool, never through Bash)

| Run | Host | Result |
|---|---|---|
| `wcore-cron --lib lease` | SeanDesktop | **6 passed / 0 failed / 0 ignored / 67 filtered out** |
| `wcore-cron --test single_owner` (incl. 2 new cross-process cases) | SeanDesktop | **11 passed / 0 failed / 0 ignored / 0 filtered out** |
| `wcore-agent --lib budget_authority` | SeanDesktop | **20 passed / 0 failed / 0 ignored / 2194 filtered out** — includes `concurrent_journal_writer_never_faults_budget_authority` |
| mandatory-lock probe | SeanDesktop | locked-range read REFUSED; unlocked range OK:8; after unlock OK:8; past-EOF lock leaves bytes readable; 2nd handle in one process REFUSED |

## Log

- (t0) worktree created, SHA asserted `b2ddf113`, branch `lane/windows-legs-sweep`.
- (t0) Windows reachable.
- (t1) Windows clone @ `37a15850`, asserted. cron lease 6/6 green.
- (t2) Added 2 cross-process lease cases; 11/11 green on Windows @ `9aa1f6f6` (asserted).
- (t3) Mandatory-locking measured product-free, both directions + self-test.
- (t4) F21-04-03 fixed arm 20/20 on Windows; reverted arm running.
