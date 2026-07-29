# ZOMBIE-PROBE — running notes (append-only; committed early per LANE-BRIEF §6b-i)

Lane `lane/zombie-probe`, base `plan/f20-unified-audit-repair` @ `797d4889`.

## T+0 — setup

- Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-zombie-probe`,
  `git rev-parse --show-toplevel` confirmed, HEAD `797d4889`.
- Read `.planning/CI-IMAGE.md` §2 in full. The mechanism is established there; I am
  not re-deriving it. Charge: fix the probes, keep it cross-platform, prove BOTH
  directions with a REAL zombie, then re-run the 13 with `--init` removed.

## T+0 — instrument hazards already hit (record, then repair — §6b-ii)

1. **`rtk` caches / rewrites git output.** `git log --oneline -3 gh/plan/f20-unified-audit-repair`
   returned `45f1a567` while `git rev-parse` on the same ref returned `797d4889`.
   `rtk proxy git log` returned the true `797d4889`. **Every load-bearing git/rg/sed
   invocation in this lane goes through `rtk proxy`.** A stale ref read is exactly the
   "gate that measured the wrong thing" class.
2. **`rg` is rewritten to `grep`** by the same hook, and `grep` rejects `--no-heading`.
   A sweep that silently degraded would have under-counted call sites. Same fix:
   `rtk proxy rg`.

## T+15 — first sweep: the site inventory is LARGER than 13

The CI-IMAGE finding named 13 *failing tests* across 4 probe sites. The 13 is a count of
**test cases**, not of code sites, and it is scoped to what went red in that CI run. The
defect (a `kill(pid,0)==0` / `/proc/<pid>` exists liveness probe that reads state `Z` as
alive) is present in more places than the four, and **including production code**.

Provisional inventory from `rtk proxy rg 'libc::kill' -g '!target'` and
`rtk proxy rg '/proc/\{' -g '*.rs'` (to be refined; `ps`, `kill -0`, `sysinfo` and
Windows `OpenProcess` sweeps still pending):

### Already zombie-aware (hand-rolled, 4 independent copies — duplication is itself the defect)

| site | notes |
|---|---|
| `wcore-mcp/src/manager.rs:1479` `process_gone_or_zombie` | `unwrap_or(true)` on malformed stat |
| `wcore-mcp/src/transport/stdio.rs:1100` `killed_or_zombie` | `unwrap_or(false)` on malformed stat — **diverges from the copy above** |
| `wcore-sandbox/src/backends/no_sandbox.rs:284` `process_running` | third copy |
| `wcore-agent/tests/dangerous_lease_e2e_test.rs:79` | fourth copy |

Four copies of the same 15 lines, already disagreeing on the malformed-input branch.
This is precisely the "centralize platform differences / no duplicate code across
crates" rule in AGENTS.md being violated, and the reason the other sites never got
the fix.

### Naive — zombie reads as ALIVE (the defect)

| site | shape | kind |
|---|---|---|
| `wcore-eval-scenarios/tests/runner_contracts.rs:127` | `kill(pid,0)` | test probe (7 of the 13) |
| `wcore-eval-scenarios/src/pty_capture.rs:785` | `kill(pid,0)` | test probe (2 of the 13) |
| `wcore-sandbox/tests/process_capture.rs:12` | `/proc/{pid}` exists | test probe (2 of the 13) |
| `wcore-swarm/src/worktree_tests/linux.rs:629` | `/proc/{pid}` exists | test probe (2 of the 13) |
| `wcore-eval-scenarios/tests/smoke.rs:131` | `kill(pid,0)` | test probe — NOT in the 13 |
| `wcore-sandbox/src/backends/process_tree.rs:850` `pid_is_alive` | `kill(pid,0)` | test helper — NOT in the 13 |
| `wcore-tools/tests/cancel_subprocess_test.rs:47` | `kill(pid,0)` | test probe — NOT in the 13 |
| `wcore-exec-backend/src/backends/local.rs:376` | `kill(pid,0)` | **PRODUCTION** |
| `wcore-gateway/src/pidlock.rs:307,313` | `/proc/{pid}` + `kill(pid,0)` | **PRODUCTION** |
| `wcore-browser/src/supervisor.rs:484` | `kill(pid,0)` | **PRODUCTION** |
| `wcore-cli/src/cron.rs:1151` | `/proc/{pid}` + `kill(pid,0)` | **PRODUCTION** |

The production four matter more than the 13 red tests: a pidlock or a cron
already-running guard that reads a zombie as a live process **deadlocks the feature**
on any host without a reaping init, which is the customer-deployment case the brief
names.

## Open questions at T+15

- Placement of the centralized helper. Every candidate crate has a wart:
  - `wcore-types` — zero internal deps, everybody can take the edge with no cycle
    risk, but it is a pure data-type crate with no `libc`.
  - `wcore-config` — the established home for cross-platform helpers
    (`wcore_config::shell`, named in AGENTS.md). Depended on by 9 of the 10 crates
    involved; **`wcore-sandbox` is the only miss** (it depends on `wcore-types` alone).
    `wcore-config` does not depend on `wcore-sandbox`, so the edge introduces no cycle.
  - `wcore-sandbox` — semantically the process-containment crate, but `wcore-mcp`,
    `wcore-gateway` and `wcore-eval-scenarios` do not depend on it.
  To be decided with the cross-audit panel.
- Windows/macOS liveness semantics. Neither has `/proc`. macOS: `kill(pid,0)` returns
  0 for a zombie too — needs `sysctl` `KERN_PROC_PID` + `SZOMB`, or `sysinfo`
  (already a dependency — see `wcore-cli/src/crash_sentinel.rs:259`). Windows has no
  zombie concept in the Unix sense: a handle keeps an exited process's PID reserved,
  and `OpenProcess` + `GetExitCodeProcess != STILL_ACTIVE` is the correct probe.
  **Must not silently become Linux-only.**
- Still to sweep: `ps `, `kill -0` (shell), `sysinfo` process lookups, `OpenProcess`.

## T+70 — helper landed, Linux proven both directions against a REAL zombie

Commit `5d202329`. Placement decided **A** (`wcore_types::process_liveness`) —
cross-audit panel codex `A`, kimi `A`, gemini `B` (gemini's objection: it taints
a data-types crate with OS bindings). Majority A. Adopted with a scope-discipline
concession to gemini: the module is one enum plus four functions, it spawns
nothing and signals nothing, and the Cargo.toml comment says so, so the crate
does not become an OS grab-bag by precedent. Decisive factor across all three
was **edge weight, not edge count**: A and B both need 3 new edges, but A's land
on a zero-internal-dep crate while B's drag tokio/cap-std/tar/which/uuid into
wcore-mcp, wcore-gateway and wcore-eval-scenarios.

### Linux — proven, hetzner-dsm, `cargo test -p wcore-types --test real_zombie`

`4 passed; 0 failed; 0 ignored` (executed count read back, not exit status).
The corpse is real. Raw `/proc/<pid>/stat` captured from the test's own stdout:

```
independent oracle for pid 3572344: 3572344 (sh) Z 3572337 3572298 ...
```

State `Z`, and the test asserted **at that same instant** that the old shape
(`kill(pid,0)==0` and `/proc/<pid>` exists) still reported it ALIVE.

### The gate can fail — mutation-proved, not asserted

Mutated `proc_stat_state_is_corpse` to `false` (exactly the old naive shape) on
hetzner and re-ran: `3 passed; 1 failed`. The one that failed was the corpse
test, with `left: Live  right: Dead`. The three positive-direction tests stayed
green, which is the second half of the proof: the fix is not universal denial.
Source restored and verified clean (`RESTORED_CLEAN`).

## T+70 — INSTRUMENT DEFECT #3, mine, found and repaired in-lane (§6b-ii)

My first cross-target check was written

```
cargo check --target $T 2>&1 | tail -25 ; echo "RC=$?"
```

and printed **`RC=0` for a check that had failed with three E0425 errors** —
`$?` after a pipeline is `tail`'s status. I wrote the canonical self-passing
gate by hand, inside the lane whose entire subject is instruments that cannot
distinguish the outcomes they exist to distinguish.

Repaired as `.planning/evidence/zombie-probe/run-capture.sh`, which runs the
command with no pipeline and prints `TRUE_RC=` plus `LOG_BYTES=`. Self-test:
**3 checks, 0 failed.**

**And the third assertion immediately caught a defect in itself.** Its first
version reproduced the broken idiom *inside* `run-capture.sh`, which sets
`pipefail` — so the pipeline returned 7 and the assertion reported "this
platform does not exhibit the defect". It does. The reproduction now runs in a
plain `bash -c` with `set +o pipefail`, matching the ssh shell where the defect
actually occurred. A self-test that reproduces a defect under settings the
defect cannot survive is a slower way of not testing.

## T+85 — macOS: the arm the libc crate cannot express, measured in C

`cargo check -p wcore-types --all-targets --target aarch64-apple-darwin` FAILED:
**`libc::kinfo_proc` does not exist for Apple targets** (libc 0.2.186, three
E0425s). `libc::proc_bsdinfo`, `libc::PROC_PIDTBSDINFO`, `libc::proc_pidinfo`
and `libc::SZOMB` (=5) all DO exist, so `proc_pidinfo` was the obvious
substitute. **Measurement disqualified it.**

`cargo check --target x86_64-pc-windows-msvc` passed clean at the same commit.

LANE-BRIEF §0 forbids cargo on the Mac and neither build host executes Darwin
code, so the macOS *semantics* were measured in C instead — `cc` is not cargo.
`.planning/evidence/zombie-probe/zombie-probe-macos.c`, raw capture in
`MACOS-PROBE-RESULT.txt`, run on this Mac:

| arm | `kill(pid,0)` (OLD shape) | `proc_pidinfo` | `sysctl` p_stat | correct? |
|---|---|---|---|---|
| A real zombie (`ps` says `Z`) | **says ALIVE** | -1, errno ESRCH | **5 = SZOMB** | sysctl only |
| B genuinely live | says alive | 2 = SRUN | 2 = SRUN | both |
| D live, **other user** (launchd, pid 1) | **says NOT alive** | **-1, errno EPERM** | **2 = SRUN** | sysctl only |
| C fully reaped | says gone | -1, errno ESRCH | rc=0, size=0 | both |

Three things this settles that guessing would have got wrong:

1. **The defect is NOT Linux-only.** `kill(pid,0)` returns 0 for a macOS zombie
   exactly as it does on Linux. A Linux-only fix would have left macOS broken
   while looking complete.
2. **`proc_pidinfo` is disqualified by ARM D.** It fails with EPERM for a
   *live* process owned by another user, indistinguishably from its ESRCH
   failure for a corpse. "libproc failed ⇒ dead" is universal denial — the exact
   trap in this lane's brief — and ARM D is the arm that exposes it. I added
   ARM D expecting it to be a formality.
3. **The old shape is wrong in BOTH directions on macOS.** It reports a corpse
   as alive (A) *and* a live process as dead (D).

ABI facts printed rather than assumed: `sizeof(kinfo_proc)=648`,
`offsetof(kp_proc.p_stat)=36`, `offsetof(kp_proc.p_pid)=40`.

Because `libc::kinfo_proc` is unavailable, the Rust arm reads those two fields
out of a raw byte buffer and **guards the assumption by reading `p_pid` back
and comparing it to the pid it asked about** — an ABI drift becomes
`Indeterminate`, never a wrong answer. That exact algorithm was then implemented
*in C* alongside the struct-typed read and run on all four arms:

```
ARM A -> rc=1 raw_p_stat=5 raw_p_pid=66984 -> DEAD(zombie)
ARM B -> rc=1 raw_p_stat=2 raw_p_pid=67038 -> LIVE
ARM D -> rc=1 raw_p_stat=2 raw_p_pid=1     -> LIVE
ARM C -> rc=0                              -> DEAD(gone)
```

4/4 correct, agreeing with the struct-typed read on every arm.
