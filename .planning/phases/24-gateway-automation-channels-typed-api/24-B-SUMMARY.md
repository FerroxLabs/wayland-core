---
phase: 24-gateway-automation-channels-typed-api
plan: "B"
subsystem: gateway
tags: [operator-verbs, service-lifecycle, drain, profile-isolation, delivery-continuity]
status: partial
plans-not-executed:
  - "24-03 — NOT STARTED"
  - "24-04 — NOT STARTED"
requires:
  - "24-01"
  - "24-02"
provides:
  - "wayland-core gateway install|uninstall|start|stop|restart|status|drain|run (the shipped binary)"
  - wcore-gateway::service::ServiceSpec::sanitised_profile (one sanitiser for the identifier and the --profile argument)
affects:
  - crates/wcore-cli/src/lib.rs (one additive `pub mod` line)
  - crates/wcore-cli/src/main.rs (one enum variant, one match arm, both adjacent to Backend)
  - crates/wcore-gateway/src/service.rs (all three families now pass --profile to the runtime they register)
tech-stack:
  added: []
  patterns:
    - "branch on a trait CAPABILITY (does this family write a unit?) rather than on the platform"
    - "the pid record is the identity, the published projection is the state, and the pid decides"
    - "run defaults to foreground because a service manager supervises the child it launched"
key-files:
  created:
    - crates/wcore-cli/src/gateway.rs
    - .planning/phases/24-gateway-automation-channels-typed-api/24-B-GATEWAY-SURFACE.md
  modified:
    - crates/wcore-cli/src/{lib,main}.rs
    - crates/wcore-gateway/src/service.rs
decisions:
  - "journey-minimal (4/4 panel): seven lifecycle verbs plus a real `run`; doctor and logs are a recorded gap asserted by test"
  - "The operator surface goes in wcore-cli where the crate's own module docs already name it, not in a separate binary"
metrics:
  tests-green: 2283
  completed: 2026-07-27
---

# Phase 24 Lane B: Gateway Operator Surface Summary

The phase's largest structural hole is closed. `crates/wcore-cli/src/gateway.rs`
now exists, the shipped binary has the gateway verbs, and the whole lifecycle —
install, start, status, hard kill, **platform-driven** recovery, drain,
uninstall — is proven live on Linux against a real `systemctl --user` service.
**Four HIGH defects were found by running it, all four fixed. 24-03 and 24-04
were not started.**

## Termination state

**Partial.** One unclaimed deliverable executed to completion and live-proven;
two claimed plans not begun. Graded honestly below rather than averaged.

## 1. This was a defect fix, not a feature

`wcore-gateway::service` generates the native unit for all three families and
every one invokes `<binary> gateway run`. There was no `gateway` subcommand.
**Every install on every platform registered a unit whose command failed with
a clap "unrecognized subcommand" error** — registration succeeded, service
never ran, silently. That reframes the whole item: 24-01 and 24-02 both
declined this file as out of scope, and what they were declining was a live
defect in already-merged code.

The wiring cost was already paid: `wcore-cli/Cargo.toml` line 115 already
carried the dependency, so **no Cargo.toml and no Cargo.lock edit was needed**
and the shared-file fence cost one `pub mod` line plus two blocks adjacent to
the existing `Backend` entries.

## 2. The cross-audited decision

**`journey-minimal`, 4/4** (codex, gemini, kimi, internal adversarial), all
four on one identical 131-line evidence bundle, all four captures retained at
`/tmp/f24b-run/gateway-decision/`. Unanimous, so no minority or tiebreak
clause applies.

Build `run` plus install, uninstall, start, stop, restart, status, drain.
**`doctor` and `logs` from the 24-01 nine-verb contract are a RECORDED GAP** —
a test asserts their absence so adding either forces the contract note to be
updated rather than silently diverging again.

The internal pass concurred with a binding condition: the saving is only real
if `run` is a real runtime, and the claim must rest on a live count across an
ungraceful kill — otherwise the honest report is that the surface exists and
Criterion 1 remains open. **That condition is reported against in §5, including
the half of it that was not met.**

Dissent recorded in full in `24-B-GATEWAY-SURFACE.md` §2. Its strongest point:
`logs` was dismissed as "diagnostics, not lifecycle", but the only in-product
evidence of an UNATTENDED relaunch is what the relaunched process wrote, so
this phase closes that clause on a narrower claim than the criterion reads.

## 3. Four HIGH defects, all found by running it

| ID | What | Status |
|---|---|---|
| F24-B-H1 | No unit passed `--profile`, so the runtime resolved `default` while the registration was named for the operator's profile. `gateway status --profile f24b` printed `profile: default`. | FIXED |
| F24-B-H2 | `status` answered "registered?" with `systemctl --user is-active`, which answers ACTIVITY — so during systemd's restart window and after a drain it reported `Uninstalled` for a unit on disk and enabled. | FIXED |
| F24-B-H3 | The runtime published nothing between accepting a drain request and finishing; the projection stayed `Running` for the whole budget. | FIXED |
| F24-B-H4 | The drain clock returned the per-iteration increment where the contract requires TOTAL elapsed, so `elapsed` was pinned at 100 and **`gateway drain` hung indefinitely with carried work**. | FIXED |

F24-B-H1 is Criterion 1's *profile isolation* clause failing directly.

**F24-B-H4 is the one worth carrying forward.** It passed the FIRST live
journey, because that gateway had zero pending deliveries so the drain loop
broke on its first observation. A green suite, a green clippy and a green live
journey all missed it. The second journey — the one that seeded actual
unsettled deliveries — caught it in a single run. **A new entry for the
standing self-passing list: a live test whose scenario was too clean to reach
the defect.** "Live-test it" is necessary and not sufficient; the live scenario
has to carry state.

### Gates proved able to go red — seven mutations, by measurement

Delete the liveness check → 1 red. Delete the pid-identity override → 1 red.
Rename the `run` verb, restoring the original defect → 2 red. Drop `--profile`
from one family → 1 red. Pass the raw profile instead of the sanitised one →
1 red. Put the registration check back on the activity query → 1 red. Return
the increment from the drain clock, restoring F24-B-H4 → 1 red. Each reddened
exactly the intended test and nothing else; each was reverted with
`git diff --stat` confirmed clean.

## 4. Verification

| Gate | Result |
|---|---|
| `cargo test -p wcore-gateway` | **45 passed, 0 failed** |
| `cargo test -p wcore-cli --lib gateway::` | **9 passed, 0 failed** |
| `cargo nextest run -p wcore-gateway -p wcore-cli -p wcore-cron -p wcore-channels --no-fail-fast` | **2283 run: 2283 passed, 9 skipped** |
| `cargo clippy -p wcore-gateway -p wcore-cli --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |

Every exit status above was captured directly into a variable before any
filtering. No gate in this lane terminates in a pipe.

## 5. Live evidence — Linux, and its exact bound

Real `wayland-core 0.12.25` release build on `hetzner-dsm`, real
`systemctl --user`, throwaway home, profile `f24b`. Full transcripts in
`24-B-GATEWAY-SURFACE.md` §5.

- `install` wrote a real unit and `is-enabled` → `enabled`; ExecStart carries
  `gateway run --profile f24b`.
- `status --json` while running: `state=running pid=1006128 uptime_secs=4
  profile=f24b deliveries_pending=12 binary_version=0.12.25`.
- Two trigger types added through the shipped binary.
- `kill -9`: systemd recorded `code=killed, status=9/KILL`,
  `Failed with result 'signal'`, then `Scheduled restart job, restart counter
  is at 1`. **RECOVERED after 5s with a new pid. Nothing in the run restarted
  it** — the restart counter is the platform's own record.
- Continuity: twelve seeded unsettled deliveries, `carried=12 (unattempted 12)`
  before the kill and `carried=12 (unknown-outcome 12)` after it. Read
  out-of-process from the journal: **12 distinct ids, 0 duplicated by
  identity.**
- `drain --budget-ms 5000` with 12 pending: `Draining (pending 12)` →
  `Drained (pending 0)`, rc=0; runtime reported **51 observations, `Forced`,
  all 12 abandoned BY NAME and recorded durably**.
- `uninstall`: unit removed, `is-enabled` → `not-found`, no residual process.

### What this does NOT prove

**No independent-sink arrival count, so the internal pass's condition is only
HALF met.** The twelve deliveries were seeded into and read back from the
gateway's own ledger. The read was out-of-process, which rules out a runtime
grading its own in-memory state, but the ledger is still the gateway's own
record and **nothing arrived at any destination** — `f24bsink` is not a
registered channel. A real arrival count needs the hermetic fixture endpoint
24-03 Task 3 owns, which was not built.

Per the condition recorded before the work began: **the verb surface exists
and is live-proven; the delivery-ARRIVAL half of Criterion 1 remains OPEN.**

## 6. macOS — the coordinator's correction is confirmed, and it still does not reach this code

A current macOS binary IS obtainable from CI artifacts without Cargo on the
Mac; verified today (`--build-info` → `0.12.25 (source 0e7e3c43…)`, `Mach-O
64-bit executable arm64`, not expired).

**It does not carry this lane's code**, proved with a discriminating probe
rather than assumed: `cron --help` prints real help, and
`--help | grep -cE "^\s+gateway"` returns `0`.

The blocker is exact and is a CI trigger, not a platform impossibility.
`ci.yml` fires only on `pull_request → main`, `push → main` and
`push → plan/f20-unified-audit-repair`; `lane/24b` is not listed, and the file
itself records that `workflow_dispatch` was considered and rejected because
GitHub exposes it only for workflows already on the default branch. Adding the
branch edits a shared CI file five lanes depend on; opening a PR is Sean's.

**Filed as a seam request** (`24-B-GATEWAY-SURFACE.md` §8): one line,
`- lane/24b` in `on.push.branches`. It unblocks the macOS rows of 24-03 and
24-04 for every lane, not just this deliverable, and the precedent is in the
file's own comment block.

## 7. What was NOT delivered — stated plainly

1. **24-03 was not started.** Its three tasks are the channel framework
   contract across ten adapters (probe, binding, media, edit/delete/reaction,
   outbound idempotency, health, plus CLI verbs), the typed client (roles,
   command idempotency, gap-aware cursor, negotiation, canary-proved support
   bundle), and a live fixture-backed matrix on two platforms. Nothing in
   `wcore-channels` or `wcore-acp` was touched. The budget went to closing the
   gateway hole and to the four HIGH defects that closing it exposed. Reported
   as not started rather than sampled shallowly.
2. **24-04 was not started.** No journey driver, no `wayland-journey` binary,
   no receipt schema, no platform receipt, no Windows TUI evidence decision.
   Its terminal acceptance was therefore never reached, and nothing was pushed,
   merged, tagged, released, or used to close an issue.
3. **`doctor` and `logs` do not exist** — the recorded gap of §2, asserted by
   test.
4. **No independent-sink delivery arrival count** — §5.
5. **No macOS and no Windows live evidence for this code** — §6.
6. **24-02's two unpassed gates were not closed.** The CONTINUATION gate reads
   `/tmp/f24-02-run/continuation-sink-{linux,macos}.ids`; those files still do
   not exist. This lane's kill-and-continue evidence is at the ledger, not at
   an independent sink, so it does not satisfy that gate — stated rather than
   claimed. The SURFACE gate (PTY capture) was not attempted at all.

## 8. Deviations

**`crates/wcore-gateway/src/service.rs` was edited, and it belongs to 24-01.**
Required by F24-B-H1: the units it generates could not tell the runtime which
profile it hosts, so the surface this lane owns could not report the truth.
The change is surgical — all three families pass `--profile`, through one
sanitiser now shared with `service_name()` so the identifier and the argument
cannot disagree — and it carries its own test.

**Shared-file fence.** `wcore-cli/src/lib.rs` took one `pub mod` line;
`main.rs` took one enum variant and one match arm, both immediately adjacent to
the existing `Backend` entries. No reformatting, no reordering, nothing
renamed. The fence asks for one contiguous block and `main.rs` structurally
needs two (the enum and the dispatch); both are minimal and adjacent to the
same anchor.

**A discrepancy in the dispatch brief, for the record.** The brief stated that
24-04's Task 2 wants a Desktop wire-contract regeneration and that its terminal
task performs a publication. Neither matches `24-04-PLAN.md` as committed here:
its Task 2 is the Windows interactive-surface evidence decision, and its Task 4
is a cross-audited acceptance whose own text states that no push, merge, PR,
tag, release or issue closure occurs. `wcore-contract generate` was not run,
and no publication was performed — but the instruction did not correspond to
the plan on disk, and a later lane should read the plan rather than the brief.

## 9. Findings ledger

| ID | Severity | Status |
|---|---|---|
| F24-B-H1 profile never reaches the registered runtime | **HIGH** | **FIXED**, with the test that catches it |
| F24-B-H2 status conflates registration with activity | **HIGH** | **FIXED**, with the test that catches it |
| F24-B-H3 drain publishes nothing while draining | **HIGH** | **FIXED**, live re-proved |
| F24-B-H4 drain clock returns the increment, not the total — hangs | **HIGH** | **FIXED**, mutation-proved |
| F24-B-M1 no independent-sink arrival count; needs 24-03's fixture | MEDIUM | BACKLOG — named, carried to 24-03/24-04 |
| F24-B-M2 `ci.yml` does not fire for lane branches, so no lane can obtain macOS/Windows artifacts for its own code | MEDIUM | SEAM REQUEST — one line, §8 of the contract doc |
| F24-B-L1 the one-byte lock sentinel survives `uninstall` | LOW | BACKLOG — arguably correct: uninstall removes the SERVICE, not the operator's home; a stale sentinel is reacquirable and the pid record IS removed |

No CRITICAL. All four HIGHs fixed with executable evidence.

## Self-Check

Every test count, exit status and transcript line above was copied from
captured tool output, not recalled. Files asserted present were verified on
disk in the lane worktree; commit subjects were read from `git log --oneline`.
The gates that do NOT pass — independent-sink arrival, macOS, Windows,
24-02's continuation and surface gates — are named as not passing.

**Self-Check: PASSED**
