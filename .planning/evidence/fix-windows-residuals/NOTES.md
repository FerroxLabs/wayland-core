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


---

## MEASURED RESULTS

### Task 1 — the lease on Windows. BOTH DIRECTIONS CLOSED.

All at asserted Windows HEAD `f923161b`, `D:\lane-fxwr\src`, target `D:\lane-fxwr\target`.
Every count read back from a `WLRC=/WLDONE` sentinel file via a separate ssh call
(LANE-BRIEF exit-status pattern), never from an ssh exit status.

| arm | `test result:` | rc |
|---|---|---|
| **control** (real `LockFileEx`) | **11 passed / 0 failed / 0 ignored / 0 measured / 0 filtered out** | 0 |
| **`always_owner`** — a lock that NEVER blocks | **8 passed / 3 FAILED** | 101 |
| **`never_owner`** — a lock that ALWAYS blocks | **2 passed / 9 FAILED** | 101 |

One build, three arms, env-gated stub inside the `#[cfg(windows)]` `mod sys`.
Mutation proven PRESENT before use (`MUTATION_MARKER_PRESENT=1`) and ABSENT after
restore (`=0`) with a live known-positive on the same search (`LockFileEx` = 6 both
times, so the zero is a real zero and not a dead instrument). Scratch clone only;
never committed. `DIRTY=0` after restore.

This satisfies the brief's actual requirement — *"A lock that never blocks and a lock
that always blocks are equally broken"* — and §3b-iii (the gate has a reachable pass
AND a reachable fail). rc 101 survived because it was written to a file inside
PowerShell; over ssh it would have collapsed to 1.

**Also run on Windows for the first time — the actual `f24-c3-h5-reload` fix:**

| suite | result |
|---|---|
| `wcore-channels --test framework_matrix` (the `StartPolicy` reload matrix) | **19 passed / 0 failed / 0 ignored / 0 filtered out**, rc=0 |

The `f24-c3-h5-reload` lane ran this on Linux only and explicitly declined to claim
Windows. It now holds on Windows.

### The residual I actually found and FIXED

`crates/wcore-cron/tests/single_owner.rs` failed `just lint`
(`cargo clippy --workspace --all-targets -- -D warnings`) on Windows with TWO
deny-level lints — `clippy::collapsible_if` and `clippy::zombie_processes`.
Reproduced on SeanDesktop at my base before touching it, fixed at `f923161b`,
re-measured green (`WLRC=0`) and the suite still **11/0/0/0**.
Neither lint suppressed; nothing weakened.

Provenance: `4d5f8ec9 test(cron): take the single-owner lease across a real process
boundary` — i.e. the previous Windows lane's own cross-process addition is what
reddened Windows CI.

### Task 1 premise correction

My brief says the reload lease fix "has never had a Windows run". **Partly false.**
`.planning/evidence/windows-legs-sweep/NOTES.md` (lane `windows-legs-sweep`) already
ran the *lease primitive* on Windows — 11/11 plus a known-negative stub at 8/3 — and
also measured the mandatory-lock hazard product-free. What that lane did NOT run, and
what was genuinely never run on Windows, is the **`f24-c3-h5-reload` fix itself**
(`StartPolicy` / `compose_registration_error` in `wcore-channels` + `wcore-cli`). That
gap is now closed (19/0 above).

### Task 3 — `ferrox-win-msvc`. PREMISE FALSE.

Brief: *"across 40 runs it served zero jobs while its status read busy=true."*

Measured over the last **40 workflow runs / 151 job rows** (`gh api`, per-job
`runner_name`, controls: known-positive `ubuntu`=87, known-negative=0):

| runner | jobs served |
|---|---|
| `sean-mac-arm64` | 10 |
| **`ferrox-win-msvc`** | **4** |
| `SEANDESKTOP` | 2 |
| unassigned (queued, empty/NULL `runner_name`) | 15 |

It is **not** serving zero. At the moment I measured, `C:\actions-runner-ferrox\bin\Runner.Worker.exe`
(PID 53460) was live with `cargo`/`rustc` children — so `busy=true` was TRUE, not phantom.
`.runner` config maps `C:\actions-runner-ferrox` → agent `ferrox-win-msvc`.
Labels are IDENTICAL to `SEANDESKTOP` (`self-hosted, Windows, X64, msvc`), so it is not
a labels mismatch either.

**The real defect: every completed Windows self-hosted job FAILED (5 of 5), always
before running a single test.** Step-level outcomes on 3 failed jobs show
`Run tests (nextest CI profile)` = **skipped** in all three, because the job dies at
`Clippy (warnings = errors)` (2 jobs) or at `Pre-build wcore-cli release binary` (1 job).
So Windows CI has been emitting **no test signal at all**.

