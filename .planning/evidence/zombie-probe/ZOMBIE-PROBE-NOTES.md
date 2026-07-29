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
