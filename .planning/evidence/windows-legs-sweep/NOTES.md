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
- (t5) F21-04-03 reverted arm 8/20 FAILED. Both directions closed.
- (t6) Phase 22 journal suites: 86 tests, 0 ignored, 0 filtered out, all pass.
- (t7) M5 + M1 negative controls closed. M1's control needed three attempts.
- (t8) Record-lock mutation: Linux 11/11 pass, Windows 5 of 11 FAIL (os error 33).
- (t9) 21-C3 corpus on Windows: 25 passed, 0 ALLOWED, 6 running pairs of 22.

## FINAL RESULTS

| # | Leg | Verdict | Fail direction proven? |
|---|---|---|---|
| 1 | Single-owner poll lease on Windows | **PASS** | **Yes**, twice |
| 2 | `F21-04-03` Windows re-proof | **PASS** | **Yes** — 8/20 with the fix reverted |
| 3 | Phase 22 M1–M5 | **M1, M4, M5 PASS; M2, M3 NOT RUN** | Yes for M1 and M5 |
| 4 | `21-C3` on Windows | **MEASURED; criterion still NOT MET** | Partial |

### Leg 1 — lease
`--lib lease` 6 passed / 0 failed / 0 ignored / 67 filtered out.
`--test single_owner` (incl. 2 new cross-process cases) **11 / 0 / 0 / 0**.
Known-negative (always-acquire stub, ONE build, two arms):
stub ACTIVE **8 passed / 3 FAILED**; stub INACTIVE **11 passed / 0 failed**.

### Leg 2 — F21-04-03
CAS is at `reducer.rs:1570`, **not `:708`** — the brief's citation is stale.
`FIXED_RAN=20 PASSED=20 FAILED=0` · `REVERTED_RAN=20 PASSED=12 FAILED=8`
`MUTATION_MARKER_HITS=1` before, `=0` after restore.

### Leg 3 — Phase 22
86 journal tests, 0 ignored, 0 filtered out, all pass.
M5 = `operating_system_releases_writer_lease_after_process_exit` (cross-process, NOT
cfg(unix)-gated) — ran and passed on Windows. Negative: stub → `FAILED 0/1`; same build
env-unset → `ok 1/0`.
M1 = `goal_journal_compat_test`, reduces the Linux-written 84,327-byte real-binary corpus
and matches the digest pinned pre-Goal-kernel at `cd5b4e9b` — a **cross-platform**
reduction identity. Negative: byte 42163 `53`→`52`, `DIRTY=1` → `FAILED 0/2`; restored →
`ok 2/0`.
**M2 and M3 NOT RUN** — need 22-01's hand-built pre/post binary pair. Not claimed.

### Leg 4 — 21-C3
`child_authority_corpus`, real `wayland-core.exe`, 209.63 s, live host-protocol arms served
real provider requests with delegated child turns arriving.
`25 passed / 0 failed / 0 ignored / 0 filtered out`; 44 COMBINATION lines = 11 × 4.
**ALLOWED 0** · REFUSED 21 · NOT-EXPRESSIBLE 18 · NO-CHANNEL 4 · UNAVAILABLE 1.
**RUNNING PAIRS 6 of 22** (Linux: 16 of 22) — i.e. **16 pairs did not run**; a skip is not
a pass. Cause, named by the harness: `portable_pty`'s ConPTY backend does not surface the
child's stdout, so `pty_capture.rs` is `#![cfg(unix)]`; with no terminal, `confirm.rs`
denies confirmable tool calls on piped stdin, so no delegated child reaches a provider turn
on the standalone-live surface. `corpus_tool` has zero running pairs, same as Linux.
Grade unchanged NOT MET; what changed is "Windows unmeasured by anyone" is now false.

### THE HEADLINE — Windows mandatory locking is a real, Linux-invisible defect class
Same mutation (extra OS lock on the readable `schedule.owner` record), same commit
`dedfedc1`:

| Host | Result |
|---|---|
| `hetzner-dsm` (Linux) | **11 passed / 0 failed** — advisory `flock` excludes no reader |
| `SeanDesktop` (Windows) | **6 passed / 5 FAILED** — `Os { code: 33 }` ERROR_LOCK_VIOLATION |

So the split of the one-byte sentinel from the readable record is load-bearing, and a
regression of that class **would pass every Linux gate in this repository**.

Product-free probe, both directions + self-test: locked range REFUSED; unlocked range in
the same run OK:8; after unlock OK:8; past-EOF lock leaves real bytes OK:8; a second HANDLE
in one process REFUSED.

### Instrument defects found in MY OWN harnesses, repaired here (§6b-ii)
1. **M1's negative control self-passed.** `[System.IO.File]::ReadAllBytes` resolves a
   relative path against the PROCESS cwd, which `Set-Location` does not change → threw →
   `$bytes` null → index 0 → corpus never touched → "corrupted" arm reduced a pristine
   corpus and reported `passed=2`. Caught only by the third assertion,
   `CORPUS_DIRTY_AFTER_FLIP=0`. Repaired: absolute paths, explicit
   `[System.Environment]::CurrentDirectory`, byte-level before/after, hard abort.
2. **Run 2 died before measuring.** `$ErrorActionPreference='Stop'` turned cargo's stderr
   warning into a terminating error. Now `Continue` + explicit `throw`/`exit 1`.
3. **The probe reported a meaningless code.** `-band 0xFFFF` on a .NET `IOException`
   recovers the CLR facility code `0x1501` (5377), not Win32 33. Replaced with exception
   identity + message, plus an F self-test.

Three attempts before M1's control measured anything. Runs 1 and 2 both exited 0.

### NOT done
M2/M3 on Windows; a real widening for 21-C3 (so `ALLOWED=0` has no control I built); the
two-real-sibling live shape for F21-04-03. Nothing weakened: no `#[ignore]`, no `#[allow]`,
no re-gating, no deleted test, no raised timeout. Every mutation was env-gated, applied to
a scratch clone only, proven present before use and absent after, and none is committed to
source. No rebase/reset/clean, no contract generate, no push to main, no PR, no tag, no
issue closed. Secret sweep on the 21-C3 log: `sk-ant`, `sk-proj`, `ANTHROPIC_API_KEY=`,
`OPENAI_API_KEY=`, `api_key":"`, `Bearer` — **0 hits each**, live control 11 for `contract`.
