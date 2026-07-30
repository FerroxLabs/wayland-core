# fix-windows-residuals — NOTES (append-only, committed after every measurement)

Lane: `lane/fix-windows-residuals`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-windows-residuals`
Base integration: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (asserted with `/usr/bin/git rev-parse`)
Build/test host: `SeanD@seandesktop` (Windows, PowerShell **5.1.26100.8875** — NOT PS7)

## Tasks
1. Windows run of the channel reload lease fix, BOTH directions.
2. Honest verdict on the Windows journey gate with the DEFAULT adapter set.
3. Diagnose `ferrox-win-msvc` self-hosted runner: 0 jobs served across 40 runs while `busy=true`.

---

## Premise checks against the brief (LANE-BRIEF: "your brief's MEASUREMENTS are probably stale")

| brief claim | verdict | evidence |
|---|---|---|
| `C:` has 167 GB free | **STALE/FALSE** | `Get-PSDrive` on SeanDesktop: C: used 1215 GB, **free 647 GB**. D: free 5413 GB, E: free 1861 GB. Someone has cleaned up since the LANE-BRIEF was written. Still working on `D:\` per the rule. |
| `ssh SeanD@seandesktop` works | **TRUE** | `hostname` → `SeanDesktop`, rc=0 |

---

## Instrument log (§6b-ii — repair in-lane, do not merely note)

**I-1. `wc -l < file` returned `0` for a file that has 41 lines.**
Observed in the first search batch: `cat file | head -40` printed 40 lines, then
`wc -l < /tmp/lane-fxwr-lease-hits.txt` in the SAME command printed `0`.
Re-tested in isolation with a 3-line known-positive and an empty known-negative:
the same broken form then returned 3 / 0 / 41 **correctly**. So it did NOT reproduce,
and I cannot attribute it to a stable tool defect — most likely transport/render level,
which is the class LANE-BRIEF §3b and the "Windows mangles help text" near-miss describe.
**Repair adopted regardless (a non-reproducing wrong number is still a wrong number):**
every count that will appear in my report is taken with `/usr/bin/wc -l <FILENAME>`
(filename as an argument, never a `<` redirect, so the output is self-identifying) and
cross-read with the Read tool straight off disk. `rtk proxy /usr/bin/wc` agreed at 41.

**I-2. zsh ate an unquoted `--include=*.rs`** → `(eval):1: no matches found`, and the
subsequent `wc` then reported on a file that did not exist. Exactly the §3b-i trap.
Repair: every `--include` glob is quoted from here on.

**I-3. `/usr/bin/cat` does not exist on this Mac** (it is `/bin/cat`). Exit 127.
Caught because I asserted the exit code. Repair: use the Read tool for file readback.

---

## Established so far

### The lease under test (Task 1)

The "flock-based" description in my brief is **imprecise in a way that matters**:
the lock is behind a `mod sys` with three arms in `crates/wcore-cron/src/lease.rs`:

- `#[cfg(unix)]` (line 406) — locally-declared `extern "C" flock`, `LOCK_EX|LOCK_NB`,
  treats `EAGAIN`(11) / `EWOULDBLOCK`(35) as contention.
- `#[cfg(windows)]` (line 442) — locally-declared `extern "system" LockFileEx` over
  **exactly one byte at offset 0**, `LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY`,
  treats `ERROR_LOCK_VIOLATION`(33) / `ERROR_IO_PENDING`(997) as contention.
- `#[cfg(not(any(unix, windows)))]` — refuses outright rather than pretending to lock.

So a Windows implementation **does exist and is not a stub**. The module doc already
anticipates the mandatory/advisory split (lines 76-81): the lock byte is deliberately
placed on a sentinel file `schedule.lock` that nothing ever reads, so Windows' MANDATORY
lock excludes no reader. `schedule.owner` is the freely-readable record and is never locked.
That is the correct design for the difference my brief flags — but **it has never been run
on Windows**, which is the actual gap.

Consumer chain: `wcore-cron::lease::ScheduleLease`
→ `wcore-agent::channel_lease::{attempt, ChannelPollLease, ChannelPollSupervisor}`
→ `wcore-cli/src/gateway.rs` (`poll_lease`, `StartPolicy` at the reload call site).

Linux evidence for the fix: `.planning/phases/24-gateway-automation-channels-typed-api/f24-c3-h5-reload-SUMMARY.md`
(lane `f24-c3-h5-reload`, merge-base `d622cb09`). Its own §10 says plainly:
"Did not measure anything on macOS or Windows … The Windows path in particular uses a
mandatory rather than advisory lock, which is exactly the kind of difference that deserves
a real run rather than an inference." That is the run I am doing.

Candidate Windows test targets (both directions live in these):
- `crates/wcore-cron/tests/single_owner.rs` — 11 tests, **0 `#[ignore]`**.
- `crates/wcore-agent/src/channel_lease.rs` `mod tests` — has an explicit
  "a live owner must exclude" (line 880) AND a re-acquire-after-release assertion (884),
  i.e. both directions are already expressed.
- `crates/wcore-channels/tests/framework_matrix.rs` — the `StartPolicy` reload matrix.

