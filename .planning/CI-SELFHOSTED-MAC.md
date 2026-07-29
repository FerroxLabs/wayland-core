---
lane: ci-selfhosted-mac
verdict: >-
  ACHIEVED, with one job moved rather than three, and the recommendation is deliberately a hybrid.
  Measured arithmetic says a single self-hosted runner is a 3.7x THROUGHPUT DOWNGRADE against the
  5-wide hosted pool (59.2 min serial for all three macOS jobs = ~3.0 jobs/hr vs 11.25 jobs/hr), so
  "self-hosted is better" is false as a general claim; its real asset is latency while idle.
  `Build (aarch64-apple-darwin)` moved to `sean-mac-arm64` for token-less `lane/**` pushes and is
  live-proven executing there (runner_id 34, queue 4.0s, run 8.1 min, artifact independently
  verified arm64 and executed). Lane-branch native arm64 coverage and the arm64 artifact are
  RESTORED at zero hosted-pool cost. The rationing this lane was warned not to rip out is intact.
jobs-moved: >-
  ONE of three. MOVED: `Build (aarch64-apple-darwin)` -> `[self-hosted, macOS, ARM64]`, on
  `lane/**` pushes only, only without an opt-in token, never on pull_request/main/integration.
  DELIBERATELY LEFT HOSTED: `CI (macos-latest)` (26.2 min median, and the job whose 12.8k tests are
  most corrupted by a non-hermetic desktop) and `Build (x86_64-apple-darwin)` (Intel compile-check
  only; moving it would serialise a second 16.5 min behind the first on a one-job-at-a-time runner).
trade-off-accepted: >-
  A token-less lane push now gets its arm64 Darwin compile and artifact from a NON-HERMETIC machine:
  a developer desktop with a real login keychain, real fsevents, the owner's PATH and installed
  tooling, and state surviving between jobs. Scoped to compile-and-upload with NO tests, because a
  compile result is near-insensitive to ambient machine state whereas the test suite is not. The
  permissive environment also UNDER-detects missing-dependency bugs by construction (in-repo
  precedent: 49 of 68 Linux failures were three absent binaries, a class a fully-equipped desktop
  can never surface). Accepted because the alternative on lane pushes today is NO macOS coverage
  at all, and because every release-adjacent artifact still comes from a hermetic hosted runner.
residual-opt-in: >-
  `[ci-darwin]`/`[ci-macos]` still gates the hosted macOS matrix on lanes, unchanged, and now also
  means "stand the self-hosted job down and give me the full hermetic hosted treatment". Still
  opt-in on lanes: the macOS TEST/LINT VERDICT (`CI (macos-latest)`: fmt, clippy, full nextest,
  audit, release smoke, eval gate) and the INTEL `Build (x86_64-apple-darwin)` compile + artifact.
  So arm64 compile regressions and arm64 binaries are caught at the lane again; macOS-only TEST
  failures and Intel compile breaks still surface one merge hop later, at integration.
new-finding: >-
  (1) HIGH, pre-existing, NOT created by this lane: the repository is PUBLIC (allow_forking=true)
  with fork-PR approval policy `first_time_contributors`, and `ci.yml`'s `ci` matrix already puts
  self-hosted Windows runners on the `pull_request` path — so a RETURNING contributor's fork PR can
  execute on Sean's machines unapproved. `default_workflow_permissions` is `write`. This lane
  refused to widen that to the Mac: the self-hosted macOS job is `push`-only. Reported, not fixed
  (changing Windows PR behaviour is out of scope and would need its own proof).
  (2) MEDIUM, in the mechanism this lane inherited: the opt-in token is matched anywhere in any
  pushed commit message, INCLUDING prose that merely mentions it. My first commit explained the
  mutual exclusion, contained the literal string, and thereby scheduled the full hosted macOS
  matrix - 3 hosted macOS jobs spent, and my own branch's next run blocked 15 min behind them.
  Measured, not theorised: DARWIN evaluated True on my own message.
  (3) A single self-hosted runner is LOWER throughput than the hosted pool; treating it as strictly
  better would have rebuilt the starvation pathology on a personal laptop.
fence-exposure: >-
  CLEAN. `git diff 75babf32 -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` = 0 bytes
  (known-positive: the same command on .github/workflows/ci.yml reports 298 insertions(+), 6
  deletions(-), so the instrument is alive). ZERO files under `crates/` changed. `release.yml` NOT
  touched. All 6 deleted lines are report-step echo text I rewrote; 0 Windows/msvc lines removed
  (the 3 changed lines mentioning Windows are all ADDED comment prose). Paths touched:
  .github/workflows/ci.yml, .planning/CI-SELFHOSTED-MAC*.md, .planning/evidence/ci-selfhosted-mac/.
status: complete
---

# Self-hosted macOS runner — supply-side repair of lane macOS coverage

Lane `ci-selfhosted-mac`. Branch `lane/ci-selfhosted-mac`, merge-base `75babf32`.
Working log with the full measurement trail: `.planning/CI-SELFHOSTED-MAC-NOTES.md`.

---

## 1. What I moved, and what I deliberately did not

I was asked for judgement rather than a default, so the headline is the negative result:
**"move everything to the self-hosted runner" is wrong, and the arithmetic says so.**

### 1.1 The serialisation arithmetic

A self-hosted runner executes **one job at a time**. Median durations of **executed** macOS jobs
(cancelled excluded — see §5.1) over a 100-run `ci.yml` window:

| job | n | median run | max |
|---|---|---|---|
| `CI (macos-latest)` | 7 | **26.2 min** | 32.7 |
| `Build (aarch64-apple-darwin)` | 4 | **16.5 min** | 23.5 |
| `Build (x86_64-apple-darwin)` | 3 | **16.5 min** | 23.4 |

- all three, serial on one runner = **59.2 min** → **~3.0 macOS jobs/hour**
- the GitHub-hosted pool, measured independently by the previous lane → **11.25 jobs/hour**

**Moving all three is a ~3.7x throughput downgrade.** The runner's asset is not capacity, it is
**latency while idle** — a live census at 2026-07-29T06:41:37Z found **39 macOS jobs queued
against 3 in_progress** on the hosted pool, while the Mac sat `busy=false`.

Demand tells the same story from the other side. Lane traffic was measured at 234 macOS jobs over
10.84 h = **~7.2 lane pushes/hr** at full fan-out. Even moving only the 16.5-min job,
7.2 × 16.5 = **119 min of work per 60 min of capacity — 2x oversubscribed**. That is why the job
carries a per-ref `concurrency` group with `cancel-in-progress: true`: a superseded commit's binary
is worthless to the lane that asked for it, and coalescing collapses a burst to one build. Without
it I would have reproduced the hosted pool's exact pathology on Sean's laptop.

### 1.2 The decision

| job | destination | reasoning |
|---|---|---|
| `Build (aarch64-apple-darwin)` | **SELF-HOSTED** on token-less `lane/**` | cheapest of the three; the ONLY job that uploads the arm64 binary, and it uploads independently of whether tests passed — exactly when a lane most needs one. Answers "does this compile on arm64 Darwin?", the most common macOS-only break. |
| `CI (macos-latest)` | **HOSTED**, opt-in on lanes | 26.2 min of a personal machine per push; and it is precisely the job whose 12.8k tests (keychain, fsevents, sandbox, process-reaping) both behave differently on a real desktop and could touch the owner's real keychain state. The canonical verdict should be hermetic. |
| `Build (x86_64-apple-darwin)` | **HOSTED**, opt-in on lanes | compile-check only, Intel, lowest urgency. Moving it would serialise a second 16.5 min behind the first for a platform in decline. |

`main`, the integration branch and **every** `pull_request` are untouched: full hermetic hosted
matrix, exactly as before. `release.yml` is fenced and untouched, so **every shipped artifact still
comes from a hosted runner** — only the binary a lane live-tests with comes from the desktop.

---

## 2. Live proof

### 2.1 A job actually executed on `sean-mac-arm64`

Run `30429811126`, commit `c6b051c3`, branch `lane/ci-selfhosted-mac`. Job-level API, not a tick:

```
name        : Build (aarch64-apple-darwin) [self-hosted]
conclusion  : success
runner_name : sean-mac-arm64
runner_id   : 34            <- matches the registered runner exactly
created_at  : 2026-07-29T07:07:06Z
started_at  : 2026-07-29T07:07:10Z      queue wait  4.0 SECONDS
completed_at: 2026-07-29T07:15:13Z      run time    8.1 min
```

All 9 steps `success`, including the two that can fail. In-log, from the runner itself:

```
runner_name=sean-mac-arm64
runner_os=macOS runner_arch=ARM64
uname_m=arm64
ProductVersion: 26.3
OK: genuine Apple silicon
```

The run scheduled **9 jobs and ZERO hosted macOS jobs** — the coverage was restored without
spending any of the contended pool.

Note the run time: **8.1 min against a hosted median of 16.5 min.** The desktop is ~2x faster than
a hosted macOS runner *even with a cold `target/`* (`actions/checkout` defaults to `clean: true`,
which wipes it — kept deliberately, §4.2).

### 2.2 The artifact is genuinely arm64

In-job, before upload — and note `file -b`, not `file <path>`: the path contains
`aarch64-apple-darwin`, so grepping un-suppressed `file` output for an arch token would match the
**directory name** and pass for a binary of any architecture. That gate would have been a tautology.

```
file -b   : Mach-O 64-bit executable arm64
lipo -archs: arm64
OK: lipo reports exactly arm64
OK: Mach-O header says arm64
wayland-core 0.12.25            <- executed natively on the runner that built it
```

Independently re-verified after `gh run download` on this Mac:

```
$ file -b wayland-core   -> Mach-O 64-bit executable arm64
$ lipo -archs wayland-core -> arm64
$ ./wayland-core --version -> wayland-core 0.12.25   (rc=0)
```

**Discriminating control**, because "lipo said arm64" is worthless from an instrument that says
arm64 for everything: `lipo -archs /bin/ls` → `x86_64 arm64e`. The tool reports other
architectures, multiple architectures, and distinguishes `arm64` from `arm64e`.

### 2.3 The counterfactual — the same work, hosted, in the same window

Both arms are pushes to the **same branch**, ~5 minutes apart, against the **same** macOS queue.
One variable: which runner the arm64 Darwin build targets.

| arm | job | created | got a runner | outcome |
|---|---|---|---|---|
| **A — hosted** (run 30429535126) | `Build (aarch64-apple-darwin)` | 06:51:11Z | 07:01:37Z — **10.4 min queued** | still running at 07:06:56Z when I cancelled; **never completed** |
| A — hosted | `CI (macos-latest)` | 06:51:11Z | **never** | `runner=UNASSIGNED` after **15.4 min** |
| A — hosted | `Build (x86_64-apple-darwin)` | 06:51:11Z | **never** | `runner=UNASSIGNED` after **15.4 min** |
| **B — self-hosted** (run 30429811126) | `Build (aarch64-apple-darwin) [self-hosted]` | 07:07:06Z | 07:07:10Z — **4.0 s** | **success in 8.1 min** |

**10.4 minutes of queue versus 4.0 seconds — a ~156x latency difference for the identical build,
minutes apart, on the same branch.** A green run would have proved nothing here; what is proved is
that the hosted route could not deliver the same artifact in the same window.

**And the pathology reproduced itself on my own branch, which is the sharpest part of the
counterfactual.** Arm B's run sat at `status=pending, jobs=0` for **11 minutes** — not queued
behind runners, but behind Arm A's per-ref concurrency group, which `cancel-in-progress: false`
holds until Arm A completes, and Arm A could not complete because its macOS jobs had no runners.
That is exactly the "concurrency group turns over at macOS speed" mechanism the previous lane
documented, observed live on my own branch. Arm B dispatched **32 seconds** after I cancelled
Arm A, and the self-hosted job started 4 s later.

### 2.4 Windows runners unaffected

- **Registry unchanged**: 3 runners; `ferrox-win-msvc` and `SEANDESKTOP` both `online`, labels
  still `self-hosted,Windows,X64,msvc`. Not touched, relabelled, stopped or reconfigured.
- **Same jobs scheduled** in my arm-B run under the new config: `CI (Array)`,
  `Build (x86_64-pc-windows-msvc)`, `Build (aarch64-pc-windows-msvc)` — all present, names
  unchanged.
- **Diff evidence**: of 306 changed lines in `ci.yml`, exactly **3** mention Windows/msvc/Array and
  **all 3 are ADDED comment prose** (`+  #`). **Zero** Windows lines removed
  (`grep -c '^-.*(windows|Windows|msvc|Array)'` = 0).

---

## 3. What a self-hosted runner costs us in hermeticity

The brief asked whether this is better or dirtier coverage. **Both, in different directions**, and
the scoping follows from which.

**Genuinely better.** Real Apple-silicon hardware with a full login keychain, real fsevents, a real
case-insensitive APFS volume, and a normal user environment — closer to what a user actually runs
than an ephemeral CI VM. macOS 26.3 on current desktop silicon, versus whatever the hosted image
pins.

**Genuinely dirtier, and this is not hypothetical in this repo:**

1. **State survives between jobs.** `ci.yml` already carries a step named *"Clear stale
   install-action state (Windows flake mitigation)"* — a recurring `CI (Array)` red caused by the
   self-hosted Windows runner leaving a locked directory behind between runs. That is the same
   machine class, the same failure mode, already costing this project red builds.
2. **A permissive machine UNDER-detects missing-dependency bugs.** The Linux CI image comment
   records **49 of 68 failures** that were nothing but three absent binaries (`python3`, `procps`,
   `bubblewrap`). A desktop with everything installed can never surface that class — it will report
   green for code that fails on a clean machine.
3. **No reimaging.** Disk, caches and toolchains accumulate on hardware nobody wipes.
4. **It is the owner's working machine.** LANE-BRIEF §0 forbids agents from running cargo on this
   Mac *because Mac builds are slow and were causing real problems*. Routing CI here does not break
   the letter of that rule, but it hands the same cost to the same machine through a different
   door. That is a direct argument against moving the 26-minute job, and I did not move it.

**What I isolated in response, rather than just noting it:**

- **No tests run on it.** Compile-and-upload only. A compile result is near-insensitive to ambient
  machine state; the 12.8k-test suite is the opposite, and is the part that could interact with a
  real keychain.
- **`clean: true` kept** (§4.2) — the one form of statefulness that is trivially removable is
  removed, at the cost of the warm-cache speedup a self-hosted runner could otherwise give.
- **Never on `pull_request`** — see the HIGH finding below.
- **Bounded**: `timeout-minutes: 45` and per-ref cancel-in-progress, so it cannot own the machine.
- **Arch is verified, not trusted** (§4.1).
- **Nothing release-adjacent depends on it**: `main`, integration, PRs and `release.yml` all stay
  hosted and hermetic.

---

## 4. Two things I hardened because the brief's history demanded it

### 4.1 The Rosetta trap, made fatal instead of trusted

The first install of this runner carried `ARM64` as a hand-written **`custom`** label while the
runtime self-reported `X64` — the x64 package under Rosetta. A job targeting
`[self-hosted, macOS, ARM64]` would have matched it and compiled and tested **x86_64 under
emulation while believing it had Apple-silicon coverage**, then uploaded that as
`wayland-core-aarch64-apple-darwin`.

The current runner self-detects all three labels (`read-only`), which I verified. But **a label is
a claim**, so the job's **first step, before checkout**, measures `uname -m` and `RUNNER_ARCH` and
**exits 1** on mismatch rather than producing a mislabelled artifact. A mislabelled runner is now
rejected before it is handed any repository code.

For the record, the same call shows `msvc` is `custom` on both Windows runners — but that is a
*tag*, not an architecture claim, and neither Windows runner asserts its arch via a custom label.

### 4.2 `clean: true` kept deliberately

`actions/checkout` defaults to `git clean -ffdx`, which wipes `target/` and forfeits the warm-cache
speedup that is usually the reason to want a self-hosted runner. **Kept anyway**: this runner's
whole liability is surviving state, so the one form of it that is trivially removable is removed.
`~/.cargo`'s registry still persists outside the workspace, which is why a cold build still lands
at 8.1 min. `clean: false` remains available as a lever; it trades correctness isolation between
lane branches for minutes, and I did not take it.

---

## 5. Instrument defects found in my own work, and repaired here (LANE-BRIEF §6b-ii)

Not written up and left — repaired in the same lane, each with the third assertion.

1. **I read `rc=0` from a CRASHED run.** I invoked my self-test as `python3 gate.py … | tee f; echo $?`
   — **the pipe stole the exit status**, the exact defect §3.2 names, committed by me while building
   a gate against that class. The suite had exited 1 on an uncaught exception. Repaired: status is
   read with no pipe.
2. **An evaluator exception aborted the remaining self-test arms.** Arm 2 raised on a `!=` clause;
   **arms 3, 4 and 5 never ran** while the suite still printed plausible output and a PASS-shaped
   tail. Repaired: `!=` modelled; an unmodelled clause is a typed `Unparsed` that becomes a gate
   FAILURE — never coerced to `False`, which would report the *safe* answer for an expression the
   gate never understood; and each arm is exception-isolated.
3. **Two mutation arms were undetected — real gate weakness, not mutation noise.**
   - *Fork safety*: my only `pull_request` case used `ref='main'`, so `startsWith(ref,'lane/')`
     excluded it incidentally and deleting the `event_name == 'push'` guard changed nothing the gate
     could see. Added a `pull_request` case with a **lane-shaped ref**, so the push-only guard is now
     the only thing producing the safe answer.
   - *Rosetta assertion*: I tested `"exit 1" in step_body`, but the step contains a **second**
     `exit 1` for `RUNNER_ARCH`, so gutting the uname branch still satisfied it. Replaced with
     `fatal_on_arch_mismatch()`, scoped to the uname branch itself.
4. **My own success message lied on failure** — "naive matcher would have missed it" printed next to
   `GATE_FAILURES=0` arms. Now only printed when the gate actually caught the mutation.
5. **My first drill-preflight self-test's third assertion tested the scaffold, not the guard** — the
   "naive matcher" ran over a synthetic string that never contained the filename. Rebuilt against
   the real `ci.yml`.

I also avoided one inherited trap rather than reproducing it: in Arm A, `started_at == created_at`
for the two macOS jobs that never ran, which naively reads as a **0.0 min queue wait**. Those jobs
were `UNASSIGNED` for the full 15.4 minutes. Reporting 0.0 there is the same cancelled-job span
defect that once produced "61 concurrent macOS jobs" where the truth was 5.

### 5.1 Gates

`.planning/evidence/ci-selfhosted-mac/gate.py` **extracts** the job's `if:` and `runs-on` from the
workflow (a hardcoded copy would be a tautology), evaluates an 11-case truth table, and enforces the
two invariants that silently destroy a run or a machine:

- **INVARIANT A — mutual exclusion.** The self-hosted job and the hosted `Build
  (aarch64-apple-darwin)` upload the **same artifact name**, and `upload-artifact@v4+` rejects a
  duplicate name within a run (409). The gate evaluates **both** conditions — mine and the previous
  lane's, read out of the same file — on every case, and fails if both are ever scheduled together.
  *(Live-confirmed in Arm A: with the token present the self-hosted job correctly reported
  `conclusion=skipped`.)*
- **INVARIANT B — fork-PR safety.** The self-hosted job must never be reachable from
  `pull_request`.

Proven able to fail by **5 mutation arms**, all detected (`SELF_TEST=PASS`, rc=0 read without a
pipe): drop the mutual exclusion (4 failures), allow `pull_request` (1), retarget to the hosted pool
(1), drop concurrency coalescing (1), make the Rosetta assertion non-fatal (1).

**Third assertion, on every arm:** the **naive matcher** — "does the file mention `self-hosted`,
`ARM64` and the job name?" — **passes all five mutations** while the real gate fails them. Without
that comparison the self-test would pass on a broken gate too.

The previous lane's gate still reports `GATE_FAILURES=0` after my change (green at base and green
after, so I broke none of its invariants — noted as a regression check, not as proof of my own work).

---

## 6. Handed-over scope: the signed release-manifest drill

`lane/release-trust-root` handed over `.github/scripts/release-manifest-drill.sh` to be wired in.

**Verified rather than taken on trust**, before wiring:
- **No credential.** `grep -nE "secrets\.|SEED|PRIVATE|_KEY|GITHUB_TOKEN|password"` → **zero
  matches**, against a live known-positive of **9** `trust-root` matches in the same file. It mints
  throwaway keys at run time into a `mktemp -d` it traps away on exit.
- **Already anti-vacuous.** It asserts the executed count against `EXPECTED_TESTS=10` and
  field-anchors its extraction rather than trusting exit status — the §3.2 discipline, built in by
  its author.
- **`bash -n` clean.**
- **Correction: it is not actually Linux-*only*.** No platform-specific commands at all
  (`apt-get|systemd|/proc|ldd|readelf|uname` → zero). "Linux-only" is a sound *placement* choice,
  not a hard constraint. I placed it on Linux as instructed.

**Placement — I made a different call than "a cheap always-on job", and here is why.** The handover
described it as "about a minute". That is true of the **drill**, but not of a **job**: the script
runs `cargo build --release -p wcore-eval-scenarios --bin wayland-release` and then compiles a
`wcore-cli` test binary. In a fresh job both are **cold** — the comparable
`Build (x86_64-unknown-linux-gnu)` release compile measures ~16 min, and ubuntu-latest is itself
congested (32 queued / 13 running). A standalone job would therefore spend **~20-40 min of a
contended pool to run a 1-minute check**.

So it is a **step at the end of `ci-linux`**, where `target/` is already warm from that job's own
pre-build and `clippy --all-targets` — marginal cost genuinely ~1 min, **zero extra jobs, zero extra
hops**. That is the same reasoning that made the previous lane revert its `budget` job.

**And it is guarded with `if: ${{ !cancelled() }}`, which is the load-bearing part.** A plain
trailing step is skipped whenever an earlier step in the job fails — and **clippy is red repo-wide
today** — so a plain step would have run **zero times while looking correctly configured**: the
always-on requirement silently unmet, which is the exact defect class this program keeps hitting.
`!cancelled()` decouples it from earlier reds; it is deliberately not `always()`, because a
cancelled job should not start a fresh compile.

The step **fails closed** if either the script or its test target is absent, with a message naming
which of the two states it is. Self-tested 3/3: known-positive passes, both known-negatives fail
(`rc=1`), and the naive matcher ("does `ci.yml` mention the drill?") **still returns True on a
guard-stripped `ci.yml`**, i.e. would have missed the regression.

**HONEST LIMITATION — I could not prove the drill green.** Its test target
`crates/wcore-cli/tests/release_manifest_pipeline.rs` is **ABSENT on my branch and PRESENT on
`lane/release-trust-root`** (established with a known-positive: `release_binary_smoke.rs` is present,
so the file-existence instrument discriminates). It lives under `crates/`, which this lane is fenced
from. I did **not** import it and did **not** copy the script into my branch — that would create an
add/add against the owning lane. **Consequence, stated plainly: on `lane/ci-selfhosted-mac` this
step is EXPECTED RED until `lane/release-trust-root` merges**, at which point it executes for real
with no further change. I am wiring, not proving, and I am not claiming otherwise.

---

## 7. New findings

### 7.1 HIGH (pre-existing, not created here) — public repo + self-hosted runners on `pull_request`

```
$ gh api repos/FerroxLabs/wayland-core --jq '{private,visibility,allow_forking}'
{"allow_forking":true,"private":false,"visibility":"public"}
$ gh api .../actions/permissions/fork-pr-contributor-approval
{"approval_policy":"first_time_contributors"}
$ gh api .../actions/permissions/workflow
{"default_workflow_permissions":"write","can_approve_pull_request_reviews":true}
```

`ci.yml` runs on `pull_request: branches: [main]`, and the `ci` job's matrix already contains
`["self-hosted","Windows","X64","msvc"]`. Approval is required only for **first-time** contributors,
so a **returning** contributor's fork PR executes on Sean's self-hosted Windows machines **with no
approval**, with `write` default token permissions.

I did not create this and I have **not** widened it: the self-hosted macOS job is `push`-only and
`lane/**`-only, enforced by a gate invariant. Fixing the Windows exposure needs its own lane — the
options (require approval for all outside contributors, or drop self-hosted from the PR matrix) both
change behaviour that other work depends on.

### 7.2 MEDIUM — the opt-in token matches prose, and it cost me a run

`contains()` scans the whole commit message, so a commit that merely **documents** the token
triggers it. My first commit explained the mutual exclusion, contained the literal string, and
scheduled the **full hosted macOS matrix**: 3 hosted macOS jobs spent on a docs-shaped change, and
my own branch's next run blocked for 15 minutes behind them. Confirmed by evaluating the real
extracted condition against my own message → `DARWIN = True`.

Not fixed here (it is the previous lane's mechanism and a narrowing could break the documented
route), but it should be: a lane writing *about* CI pays for macOS jobs it never wanted, and risk 2
in that lane's own report — reflexive opt-in restoring the original problem — has an accidental
path nobody costed.

### 7.3 The supply-side finding itself

A single self-hosted runner is **lower throughput** than the hosted pool (~3.0 vs 11.25 jobs/hr).
Adding runners does not fix an oversubscribed queue unless the per-runner throughput is comparable;
what this one buys is **latency at low demand**. Treating "self-hosted" as strictly better would
have moved all three jobs and rebuilt the starvation pathology on a laptop.

---

## 8. Fences and what I did NOT do

- **Shared-file fence CLEAN**: `git diff 75babf32 -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` → **0 bytes**. Known-positive: the same command on `ci.yml` reports
  **298 insertions(+), 6 deletions(-)**. All 6 deletions are report-step echo text I rewrote.
- **Zero `crates/` files changed. `release.yml` NOT touched** (fenced to `lane/release-trust-root`).
- **The runner was not unregistered, reconfigured, relabelled or stopped.** Read-only `gh api` only.
- **Run cancellations: exactly one, `30429535126`, on my own lane branch**, to release the
  concurrency group my own token misfire had blocked. No other lane's run was cancelled.
- **Not done (Sean-reserved):** no merge to `main`, no PR, no tag, no release, no GitHub issue
  closed, no `wcore-contract generate`.
- **Injection safety**: the new `if:` uses `head_commit.message` / `commits.*.message` only as
  operands of `contains()` inside the expression evaluator — they never reach a shell, and the
  expression can only select between literals written in the file. No shell interpolation was added
  anywhere.
- Every number here comes from `gh api` or from `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/awk`,
  `/usr/bin/env python3` by absolute path, per LANE-BRIEF §3b. Pushes were verified by comparing
  `git ls-remote gh` against local `HEAD`, never by exit status.

## 9. Recommended follow-ups (not done here)

1. **Fix the fork-PR exposure (§7.1)** — it is the highest-severity thing this lane found and it
   predates it.
2. **Narrow the token match (§7.2)** — e.g. require it on the tip commit's first line.
3. **Measure whether a second lane-visible job fits.** At 8.1 min rather than the hosted 16.5, the
   duty-cycle arithmetic is friendlier than I assumed: 7.2 pushes/hr × 8.1 min = 58 min/hr, right at
   capacity *before* coalescing. `Build (x86_64-apple-darwin)` is the candidate; it needs a
   measurement of real post-coalescing demand, not this estimate.
4. **Revisit `clean: false`** if build latency ever matters more than inter-branch isolation.
