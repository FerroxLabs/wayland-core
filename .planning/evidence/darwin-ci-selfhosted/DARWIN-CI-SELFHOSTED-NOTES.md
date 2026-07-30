# NOTES — lane/darwin-ci-selfhosted (working log, appended as measured)

Branch `lane/darwin-ci-selfhosted`. Base / merge-base: `4caaa31c891c0d606e5de1e91cdcd3e5a79ab767`
(`chore(contract): regeneration #4 over the merged tree`, 2026-07-30 12:33:42 +0700).

Committed early per LANE-BRIEF §6b-i. Appended after every measurement, not at the end.

---

## T0 — The brief's central premise is STALE. Measured first, per LANE-BRIEF "your brief's
## MEASUREMENTS are probably stale".

The dispatch brief says the Mac runner exists but no job points at it, and that "the work is to
point jobs at it". **That is false at my base.** A previous lane, `lane/ci-selfhosted-mac`, has
already landed a self-hosted macOS job and it is merged into integration.

Evidence at base `4caaa31c`:

- `.planning/CI-SELFHOSTED-MAC.md` exists, `status: complete`, verdict `ACHIEVED`.
- `/usr/bin/grep -n "self-hosted" .github/workflows/ci.yml` → **30 hits** (written to a file and
  read with the Read tool, per §3b — not read through the Bash-proxied stdout).
- `ci.yml:959` defines job `build-darwin-selfhosted`,
  `ci.yml:960` `name: Build (aarch64-apple-darwin) [self-hosted]`,
  `ci.yml:997` `runs-on: [self-hosted, macOS, ARM64]`.
- It is live in production RIGHT NOW: `gh api .../actions/runners` reports
  `34 sean-mac-arm64 status=online busy=true labels=self-hosted,macOS,ARM64`.
  `busy=true` is the load-bearing word — the runner is executing a job, not merely registered.

So the goal as literally stated ("give the macOS CI jobs a self-hosted runner") is **already
partly delivered**. What remains is the increment the previous lane deliberately deferred, and
its own follow-up #3 names it and names the missing measurement:

> "Measure whether a second lane-visible job fits. At 8.1 min rather than the hosted 16.5, the
> duty-cycle arithmetic is friendlier than I assumed: 7.2 pushes/hr x 8.1 min = 58 min/hr, right
> at capacity *before* coalescing. `Build (x86_64-apple-darwin)` is the candidate; it needs a
> measurement of real post-coalescing demand, not this estimate."

**That measurement — real post-coalescing duty cycle of `sean-mac-arm64` — is the deliverable
this lane can actually add.** Not re-doing the wiring.

## T0b — Brief constraint 3 is ALSO wrong, and this matters for the decision

The brief says: "`x86_64-apple-darwin` cannot run natively on an arm64 Mac without Rosetta. Do not
silently move that job to a runner that cannot execute its artifact."

`ci.yml:832` pins `{"os":"macos-latest","target":"x86_64-apple-darwin"}`. **`macos-latest` is
itself Apple silicon** (GitHub moved the label to arm64 at macos-14). And the `build` job has NO
run/test step for the produced binary — `ci.yml:884-891` is `cargo build --release --target ...`,
then `ci.yml:899` uploads it. It is a **cross-compile-and-upload** job on an arm64 host today.

So the hosted runner **also cannot execute that artifact**. Moving the job to `sean-mac-arm64`
forfeits zero execution coverage, because there is none to forfeit. Verified below in T2.

## T1 — DUTY CYCLE. The previous lane's missing measurement, now taken. 84.8%.

Method: 200 `ci.yml` runs (2 API pages), every job with `runner_id==34` extracted to
`machjobs*.tsv`, arithmetic in `/usr/bin/env python3`, output redirected to a file and read
with the Read tool (never through Bash stdout — §3b).

**I got this wrong the first time and the error is instructive.** A ONE-page (100-run) scan gave:

```
EXECUTED 12   window 03:30..07:00 = 210.9 min   busy 93.8   DUTY CYCLE 44.5%
```

44.5% reads as "half the machine is free — a second job fits". It is an artifact. Jobs belonging
to runs older than the 100-run page were invisible, so the runner looked idle during windows it
was in fact working. The tell was a **43.2-minute gap that a job created 2.5 h earlier did not
dispatch into** — a contradiction I could not explain, which is what prompted the second page
rather than shipping the number.

Two pages (200 runs):

```
unique runner-34 job records : 39
EXECUTED to completion       : 37
window   : 07-30 01:35..07:23 = 348.2 min (5.80 h)
busy     : 295.3 min
DUTY CYCLE : 84.8 %          <- one-page estimate said 44.5%
throughput : 6.38 jobs/hr
duration   : median 7.92 min, max 10.58
gaps > 10 min: exactly ONE (14.3 min)
```

**Sean's personal Mac is already executing CI 84.8% of the wall clock, with a single >10-minute
idle gap in 5.8 hours.** There is ~15% headroom, not 55%.

## T2 — the delay is self-hosted-specific, not a run-level hold

Full job timeline of run `30513699696` (lane/slack-live), one run, one dispatch instant:

| job | started after |
|---|---|
| Browser live e2e (ubuntu) | 2 s |
| Build (x86_64-unknown-linux-gnu) | 2 s |
| Build (aarch64-pc-windows-msvc) | 2 s |
| Eval acceptance gate (Linux) | 2 s |
| CI (linux-containerized) | 10 s |
| **Build (aarch64-apple-darwin) [self-hosted]** | **151.3 min** |
| **CI (Array) — self-hosted Windows, SEANDESKTOP** | **182.6 min** |

Every hosted job dispatched in 2-10 seconds. Only the two SELF-HOSTED jobs queued, for
2.5-3 hours. So the run was NOT held by the run-level concurrency group
(`ci.yml:47-53`, `cancel-in-progress: false` on pushes) — the run was live the whole time.

## T3 — the brief's MODEL is false, not just its file:line claims

The brief's goal is worded "give the macOS jobs a self-hosted runner, **the way Windows already
has one, so they run on every push instead of being rationed**". Measured over the last 40 runs,
self-hosted **Windows** waits: median **71.8 min**, max **182.6 min**, 2 of 3 over an hour.

**Self-hosted is not un-rationed. It is a different, and currently worse, queue.** Copying the
Windows arrangement onto macOS copies a queue problem.

Also: of the 3 registered runners, `ferrox-win-msvc` served **0** wayland-core jobs across 40 runs
while reporting `busy=true` — it is working for a different repository. wayland-core effectively
has ONE Windows self-hosted runner and ONE macOS one, not three.

## T4 — the Intel Darwin job's unique detection surface is EMPTY

`Build (x86_64-apple-darwin)` is the obvious candidate to move (the previous lane's own follow-up
#3 names it). Measured whether it can catch anything the arm64 build cannot:

```
cfg(target_arch = ...) attributes under crates/   : 0   (grep exit=1)
core::arch::x86 / std::arch::x86 / asm! / is_x86_feature : 0
KNOWN-POSITIVE CONTROL: cfg(target_os = ...)      : 64 files
```

The control proves the matcher and the regex form are alive. Every raw `target_arch` hit in the
tree is the *identifier* `target_architecture` — a release-receipt struct field — plus one doc
comment. `target_triple_for()` in `self_update.rs` is runtime string mapping, not conditional
compilation.

**Nothing in this workspace compiles differently for x86_64 vs aarch64 Darwin.** The Intel job's
residual value is dependency-side arch-conditional code only, which still runs unconditionally on
`main`, integration and every PR. Its marginal value on a lane push is near zero, and it would cost
~8 min of a runner that has ~15% headroom.

## T5 — decision

Adding ANY second macOS job to runner 34 is wrong: 84.8% + ~8 min/job is saturation, and the
observable consequence is already visible as 85-151 min waits. The brief's own constraint 1 —
"do not silently move all macOS load onto Sean's development machine" — is settled by the duty
cycle, not by preference.

The binding constraint is **throughput, not job placement**. The one lever that changes it is
per-job cost: the job currently rebuilds from cold every time because `actions/checkout` defaults
to `clean: true` (`ci.yml:1043-1052`), which the previous lane kept deliberately and listed as
follow-up #4: *"Revisit `clean: false` if build latency ever matters more than inter-branch
isolation."* **Latency now demonstrably matters.**

## Open questions this lane must still answer

1. Real duty cycle of runner 34 over the window since the job landed — not the 7.2 pushes/hr
   estimate. Needs executed-job durations + a busy-fraction, cancelled runs excluded (the
   cancelled-span defect that once produced "61 concurrent macOS jobs").
2. Does moving a second job fit inside that measured duty cycle, on a machine Sean works on?
3. A real Actions run proving whatever I change actually executes — a YAML diff is not evidence
   (`lane/glibc-reach` precedent).
