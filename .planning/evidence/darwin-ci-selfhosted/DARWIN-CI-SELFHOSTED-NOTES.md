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

## Open questions this lane must still answer

1. Real duty cycle of runner 34 over the window since the job landed — not the 7.2 pushes/hr
   estimate. Needs executed-job durations + a busy-fraction, cancelled runs excluded (the
   cancelled-span defect that once produced "61 concurrent macOS jobs").
2. Does moving a second job fit inside that measured duty cycle, on a machine Sean works on?
3. A real Actions run proving whatever I change actually executes — a YAML diff is not evidence
   (`lane/glibc-reach` precedent).
