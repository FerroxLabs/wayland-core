---
phase: 23B-continuous-agency
plan: "04"
subsystem: multi-day-journey-and-clock-policy
status: partial-clock-started
requirements:
  - F23-05
requirements_disposition:
  F23-05: in-progress
tags: [multi-day, wall-clock-authority, budget, goal, cross-audit, live-evidence]
provides:
  - crates/wcore-agent/tests/multi_day_journey_test.rs
  - scripts/f23-clock-probe.sh
  - scripts/f23-clock-panel.sh
  - scripts/f23-multi-day-journey.sh
  - scripts/f23-multi-day-journey.ps1
  - .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md
key-files:
  created:
    - crates/wcore-agent/tests/multi_day_journey_test.rs
    - scripts/f23-clock-probe.sh
    - scripts/f23-clock-panel.sh
    - scripts/f23-multi-day-journey.sh
    - scripts/f23-multi-day-journey.ps1
    - .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md
    - .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md
    - .planning/phases/23B-continuous-agency/23B-04-JOURNEY-HANDOFF.md
    - .planning/phases/23B-continuous-agency/evidence/23B-04-clock-probe-linux.log
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-bundle.md
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-codex.txt
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-gemini.txt
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-kimi.txt
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-internal.txt
    - .planning/phases/23B-continuous-agency/evidence/23B-04-panel-manifest.txt
  modified: []
commits:
  - 9be07203
  - 85d08241
  - 0ed05322
  - 07a06735
  - f4935532
---

# Phase 23B Plan 04: Multi-Day Journey — the started-clock dispatch

**Task 1 is COMPLETE. Task 2 is STARTED on Linux and running. Task 3 is
DELIBERATELY UNSTARTED.** This lane was dispatched early, before its declared
dependency 23B-03 had landed, for one reason: Success Criterion 5's floor is
three real calendar days and that is the only cost in this program that cannot
be compressed later. The clock is now running.

**Termination state: none of the plan's four.** The plan's termination criterion
assumes one session runs all three tasks. This dispatch was scoped by its
operator to Task 1 plus the start of Task 2, so the honest report is
**partial-and-running**, not a termination. Inventing a fifth state would be the
"termination state 4" move the brief names as a failure; this instead says
plainly which tasks ran and which did not.

## The two numbers that matter

```
Linux day one   2026-07-27T14:21:19Z   on hetzner-dsm, pinned SHA 0ed05322
Earliest close  2026-07-30T14:21:19Z   day one + the authorized 259,200 seconds
```

---

## Task 1 — the clock policy: measured, cross-audited, committed

### The determination ran FIRST and removed two options before the panel argued

`scripts/f23-clock-probe.sh` on `hetzner-dsm` against the release binary at the
SHA under test, nonce-bound, exit 0. Each experiment armed durable budget
authority in one real process, let that process EXIT, and bound it again in a
SECOND real process.

| Experiment | Setup | Observed |
|---|---|---|
| B (control) | absolute deadline, 20s cap, **no** gap | `exceeded=false` |
| A | identical durable state, **45s real gap** | `exceeded=true reason=max_wall_time` |
| C | active runtime, **45s real gap** | `exceeded=false` |
| D | 13 candidate env seams + the binary's `--help` | every seam inert, `0` clock flags |

A and B differed on nothing but real elapsed time, so the experiment
discriminated:

```
F23_04_ABSDEADLINE_EVAL=system-clock-at-evaluation
F23_04_ABSDEADLINE_CHARGES_DOWNTIME=true
F23_04_ACTIVERUNTIME_CHARGES_DOWNTIME=false
F23_04_CLOCK_INJECTION_SEAM=none
F23_04_ACCEL_HONEST_FOR_ABSOLUTE_DEADLINE=false
```

**Experiment D's answer is `none`.** That is the load-bearing measurement: with
no seam, both accelerated options argue about a mechanism the product does not
have.

### Where the probe contradicted the plan, and the probe won

The plan assumed the absolute-deadline form could be driven "against the SHIPPED
binary through 23B-01's session verbs". Measured: it cannot.
`BudgetWallClockAuthority::AbsoluteDeadline` has **zero** production
construction sites — `bootstrap.rs:194`, `engine.rs:22926`, `recovery.rs:1509`,
`spawner.rs:3152`, `spawner.rs:3227`, `tool_budget.rs:354` all hardcode
`ActiveRuntime`, and `BudgetConfig` has no deadline field. The probe therefore
exercised the form through `wcore-agent`'s public API from a real two-process
harness and **records that distinction rather than blurring it**. Finding
23B-04-M1.

### The panel split three ways

One bundle, 7,049 bytes, verbatim to all four members; every reply captured
verbatim with its hook noise intact; five-line run-time manifest verifies.

| Member | Position |
|---|---|
| codex (gpt-5.6-sol) | `real-time-full` |
| gemini (3.1-pro-preview) | `escalate` |
| kimi (K3) | `accelerated-except-absolute-deadline` |
| internal (adversarial) | `real-time-full` |

Three different external positions, so **no majority was arithmetically
available**. `real-time-full` is committed to on `minority-stronger-evidence` at
two of four, with `decision_basis_evidence` citing the probe log's `seam=none`.
Both dissents are recorded in their own terms in
`23B-04-CLOCK-DECISION.md` — kimi's is the strongest argument on the table and
is worth reading before anyone revisits this.

**Escalation was available and was NOT taken.** The plan defines deadlock as no
option drawing three **and** no minority carrying stronger evidence. The second
conjunct fails. This was decided, not parked.

### Accepted evidence cost

Three real calendar days minimum, two hosts occupied, and a defect found on day
three costs another full cycle. In exchange no span assertion is trivially
satisfiable and the wait condition is falsifiable on every resume.

### All eight Task 1 gates pass, and were proved able to fail

Negative controls actually run: claiming `majority` on a tally of 2 → red;
appending one byte to a panel capture after it was cited → both the sha256
cross-check and `shasum -c` go red.

---

## Task 2 — started, running, not finished

**Linux day one recorded**, all six invariants PASS, Goal parked on
`Waiting { Event { "f23-span-elapsed-259200s" } }`, journal cursor `seq=7`,
process gone afterwards. Days 2 and 3 are on systemd timers so the span cannot
be lost to nobody invoking it; each day is idempotent.

**Windows: day one NOT started.** The cold release build was still in flight at
hand-off. It was also started at the previous SHA and must be moved to the
pinned one before day one, or provenance fails.

**macOS: OPEN, blocked.** `scripts/f23-macos-binary.sh` was never landed by
23B-01 or 23B-02; the plan instructs stopping and recording rather than
improvising a second resolver, and that is what happened. Two measured
corrections go with it, in `23B-04-LIVE-EVIDENCE.md`: `ci.yml` **does** now
upload macOS release binaries and its `build` job has no `needs:` on the failing
contract-drift job, so the plan's "no binary is reachable" claim is out of date —
**but the leg is still blocked, on the TEST HARNESS**, which CI does not upload
and which cannot be built on the Mac. The route to unblock edits `ci.yml`, which
is outside this plan's declared files, so it is named and left as someone's call.

### The bug the early gate caught, which would otherwise have cost three days

The harness prints through the libtest runner, which prefixes the first marker
on a line with `test f23_journey_step ... `. Every consumer — the driver's own
span extraction and every one of the plan's gates — anchors at column one, so
they matched nothing and `--verify` reported "could not read the run log's own
first and last timestamps" on a log that plainly contained them. Same defect
class as an anchored regex losing a bullet-prefixed panel vote.

**It was found by running the span gate on day one instead of waiting for day
three.** Both drivers now normalize on the way in and read unanchored anyway,
and the pipeline status is taken from `PIPESTATUS[0]` rather than from the
normalizer, which always succeeds. The journey was restarted at the fixed SHA —
costing thirty minutes rather than three days.

### Gate discipline: every gate was made to go red at least once

| Control | Result |
|---|---|
| duplicate delivery asserted to SUCCEED | suite RED |
| cumulative budget carry-forward dropped | suite RED |
| wait completed on the first resume | suite RED |
| authorized span 999999 vs a 20s run | exit **72**, `SPAN_MEETS_AUTHORIZED_POLICY=false` |
| decision record absent | exit **70** |
| empty run log | exit **71** |
| mismatched `--sha` | exit **68** |
| baseline restored | green |

### Verification actually run

- `cargo nextest run -p wcore-agent --profile ci --test multi_day_journey_test --no-tests=fail --no-fail-fast` — **3 tests run: 3 passed, 0 skipped** at the pinned SHA on `hetzner-dsm`.
- `cargo clippy -p wcore-agent --tests -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean on the Mac.
- Full dry-run cycle at the pinned SHA with a 20s span: day 1 → day 2 (wait correctly still pending) → day 3 (terminal transition) → verify PASS, and Task 2's day-row, terminal, six-invariant, loop-owner and wait-condition gates all pass against the captured verify log.

**The committed regression test is NOT the multi-day evidence.** It runs the
whole cycle inside one process at a compressed span; it never dies as a process
and never elapses days. The multi-day evidence is the run log's own first and
last timestamps and nothing else. The test file says so in its own header.

---

## Task 3 — deliberately unstarted

It authenticates 23B-02's and 23B-03's dispositions, and 23B-03 was being built
by a concurrent lane. Starting it would have meant authenticating a disposition
that did not yet exist. Nothing in this lane records a phase outcome, a Success
Criterion verdict, an F23-05 disposition, or anything about D2.

**Verified rather than assumed** that Tasks 1 and 2 do not depend on 23B-03:
23B-03 is `wcore-repomap` indexing and retrieval (F23-06); this plan's six
invariants are budget authority, Goal lifecycle, session journal and delivery,
and its `read_first` list names no 23B-03 artifact. Only Task 3's `read_first`
cites `23B-03-LIVE-EVIDENCE.md`.

---

## Deviations from plan, with reasons

- **Task 1's and Task 2's hetzner gates check out into `/root/wayland` in the
  plan.** Run instead in an isolated worktree `/root/wayland-23B-04`, with
  `test "$(git rev-parse HEAD)" = "$SHA"` asserted explicitly. Five lanes share
  that host; a `git checkout --detach` in the shared checkout would corrupt a
  concurrent lane's build mid-run. The assertion the gate exists for is
  preserved verbatim.
- **The plan's gates `git fetch origin plan/f20-unified-audit-repair`.** The
  journey runs at a lane commit, so the fetch is `origin lane/23B-04`. The
  binary's own `--build-info` provenance assertion is what actually binds the
  evidence to the tree, and it is unchanged.
- **The clock probe's experiments A/B/C run through a compiled test binary, not
  through the shipped `wayland-core`.** Forced by measurement, not preference:
  the absolute-deadline form has no production construction site. Recorded as
  finding 23B-04-M1 rather than presented as equivalent.
- **The probe harness and the journey harness share one file,
  `crates/wcore-agent/tests/multi_day_journey_test.rs`,** which the plan
  declares under Task 2 only. One implementation of the invariants was worth
  more than staying inside the file boundary; a second implementation could
  describe a journey that never happened.
- **`23B-04-JOURNEY-HANDOFF.md` is a new file not in `files_modified`.** A
  running three-day journey with no operational handoff is a journey that gets
  restarted from zero by the next agent.
- **`memory-recall` is observed over the durable session journal, not over
  `wcore-memory`'s SQLite store.** Recorded as finding 23B-04-M2 rather than
  claimed as proof of 23B-02's subsystem.

## Findings

| ID | Severity | Finding |
|---|---|---|
| 23B-04-M1 | MEDIUM | `BudgetWallClockAuthority::AbsoluteDeadline` has **zero** production construction sites. No CLI flag, config key or env var reaches it; `BudgetConfig` has no deadline field. It is defended by the reducer's "may only tighten, never widen" invariant and by `same_wall_clock_semantics` in `restore`, and no user can reach any of it. Not HIGH because nothing a user can currently do is broken by it — it is dead capability, not a live defect — but it means the absolute-deadline half of this plan's own premise is presently unreachable. |
| 23B-04-M2 | MEDIUM | The journey's `memory-recall` invariant runs over the session journal, not `wcore-memory`. Durable recall across a real restart is proved; 23B-02's memory subsystem specifically is not. |
| 23B-04-M3 | MEDIUM | `23B-04-PLAN.md`'s macOS-binary reasoning is out of date: `ci.yml:484-491` uploads `wayland-core-<target>` for six targets including both Darwin ones, and the `build` job has no `needs:` on the failing contract-drift job. The plan's conclusion (macOS blocked) still holds, but for a different reason — the test harness, not the product binary. |
| 23B-04-M4 | MEDIUM | The libtest prefix defect described above. Fixed in `0ed05322` before it could cost a span, and recorded because the same class will recur wherever a marker is grepped at column one out of test-runner output. |

No CRITICAL or HIGH finding is open.

## What is honestly NOT proved

- Success Criterion 5 is **not met and not claimed**. One platform has one day of
  three; one has none; one is blocked.
- F23-05 is **in progress**, not complete.
- Nothing about Phase 23B's outcome, Success Criteria 2–6, or D2.
- No Windows or macOS journey evidence of any kind exists.

## Self-Check: PASSED

All fifteen created files exist on disk. All five commits resolve in `git log`.
The Linux run log, the journey root and both systemd timers were read back from
`hetzner-dsm` after they were written.
