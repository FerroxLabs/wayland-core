---
lane: darwin-ci-selfhosted
verdict: >-
  GOAL ALREADY MET BEFORE THIS LANE STARTED, and the brief's remaining ask is REFUTED by
  measurement. `Build (aarch64-apple-darwin) [self-hosted]` was already wired to `sean-mac-arm64`
  and live in production at my base — I re-proved it end to end (run 30523806783, runner_id 34,
  queue 3 s, 8.00 min, 9/9 steps success, artifact independently verified arm64 and executed).
  The brief's actual instruction — point MORE macOS jobs at the Mac "so they run on every push" —
  is measurably wrong: the runner is at 84.8% duty cycle over 5.8 h (37 jobs, union-computed,
  0 overlaps), and the candidate job to move, `Build (x86_64-apple-darwin)`, provably cannot
  detect anything the arm64 build misses (ZERO `cfg(target_arch)` attributes repo-wide against a
  live 64-file `cfg(target_os)` control). So I moved NOTHING and changed no workflow file. The
  deliverable is the measurement, the refutation, and the live re-proof.
jobs-moved: >-
  ZERO. `.github/workflows/ci.yml` is byte-unchanged by this lane (0 files under `.github/` or
  `crates/` in my diff). Current placement, unchanged and now evidence-backed: arm64 Darwin
  compile+artifact SELF-HOSTED on token-less `lane/**` pushes; `CI (macos-latest)` and
  `Build (x86_64-apple-darwin)` HOSTED and opt-in on lanes; `main`, integration and every PR keep
  the full hermetic hosted matrix.
brief-claims-refuted: >-
  FOUR. (1) "The Mac runner already exists and is online. The work is to point jobs at it" — a job
  was ALREADY pointed at it (ci.yml:959-1118, landed by lane/ci-selfhosted-mac) and executing in
  production; `busy=true` at my first API call. (2) Constraint 3, "x86_64-apple-darwin cannot run
  natively on an arm64 Mac ... do not move that job to a runner that cannot execute its artifact"
  — `macos-latest` IS arm64, and the job has no run step at all; it is already a
  cross-compile-and-upload on an arm64 host, so moving it would forfeit zero execution coverage.
  The real reason not to move it is duty cycle plus zero detection value, not executability.
  (3) The brief's MODEL, "the way Windows already has one, so they run on every push instead of
  being rationed" — self-hosted Windows waits measured median 71.8 min, max 182.6 min. Self-hosted
  is not un-rationed; it is a different and currently worse queue. (4) "three runners" —
  `ferrox-win-msvc` served ZERO wayland-core jobs across 40 runs while reporting `busy=true`; it
  is working for another repository, so wayland-core effectively has one Windows and one macOS
  self-hosted runner.
new-finding: >-
  (1) The single most load-bearing number in this lane was WRONG on first measurement and would
  have inverted the decision. A 100-run scan gave a 44.5% duty cycle, which reads as "half the
  machine is free, a second job fits". Widening to 200 runs gave 84.8%. The one-page scan was
  blind to jobs belonging to older runs, so the runner looked idle during windows it was working.
  (2) At 84.8% duty the binding constraint is THROUGHPUT, not job placement; the only lever is
  per-job cost (`clean: true` forces a cold build every run). Cross-audit split 2-1 AGAINST taking
  that lever now — recorded as a costed follow-up, not done, because it changes the provenance of
  the artifact every other lane live-tests with and cannot be proven safe inside this lane.
fence-exposure: >-
  CLEAN. `git diff 4caaa31c -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` = 0 bytes
  (known-positive: the same command on this lane's evidence dir reports 11 files, 468 insertions,
  so the instrument is alive). Files changed under `crates/` or `.github/`: ZERO. Paths touched:
  `.planning/DARWIN-CI-SELFHOSTED.md` and `.planning/evidence/darwin-ci-selfhosted/` only.
status: complete
---

# Darwin CI on the self-hosted Mac — a supply measurement that says "add nothing"

Lane `darwin-ci-selfhosted`. Branch `lane/darwin-ci-selfhosted`, merge-base `4caaa31c`.
Working log: `.planning/evidence/darwin-ci-selfhosted/DARWIN-CI-SELFHOSTED-NOTES.md`.

---

## 1. The brief's premise was stale, and its remaining ask is wrong

The brief asked me to give the macOS jobs a self-hosted runner "so they run on every push instead
of being rationed", and told me to re-verify its measurements. Both halves failed verification.

**A macOS job was already self-hosted and running.** `ci.yml:959` `build-darwin-selfhosted`,
`ci.yml:997` `runs-on: [self-hosted, macOS, ARM64]`, landed by `lane/ci-selfhosted-mac` and merged
into integration before I started. My first API call found runner 34 `busy=true`.

So the only real question was the one the previous lane explicitly deferred to a successor
(its follow-up #3): **does a second macOS job fit on that runner?** It named the missing
measurement — "real post-coalescing demand, not this estimate". I took it.

## 2. The measurement, and the mistake inside it that matters more than the result

Every job with `runner_id == 34` across 200 `ci.yml` runs, arithmetic in `python3`, output
redirected to a file and read with the Read tool (never through Bash stdout, per LANE-BRIEF §3b).

**First attempt, 100 runs — WRONG:**

```
EXECUTED 12   window 210.9 min   busy 93.8   DUTY CYCLE 44.5%
```

44.5% says the machine is half free and a second job fits comfortably. **It is an artifact of the
page boundary**: jobs belonging to runs older than the 100-run page were invisible, so the runner
appeared idle during windows it was working. I caught it only because the data contained a
contradiction I could not explain — a **43.2-minute idle gap that a job created 2.5 hours earlier
did not dispatch into**. A job cannot wait through an idle runner. Rather than ship the number, I
widened the window.

**Second attempt, 200 runs:**

```
unique runner-34 job records : 39      EXECUTED : 37
window   : 07-30 01:35..07:23 = 348.2 min (5.80 h)
busy     : 295.3 min       DUTY CYCLE : 84.8 %      throughput : 6.38 jobs/hr
duration : median 7.92 min, max 10.58      gaps > 10 min : exactly ONE (14.3 min)
overlaps : 0   (union-computed busy == sum-computed busy, so no double counting)
```

**Sean's personal Mac is already executing CI 84.8% of the wall clock**, with one idle gap over
ten minutes in almost six hours. The headroom is ~15%, not ~55%.

This is the whole decision. A second ~8-minute job at 6.4 jobs/hr demand does not fit in 15%
headroom; it produces unbounded queue growth on a machine its owner is trying to work on.

## 3. And the job we would have moved detects nothing

Independently of capacity — `Build (x86_64-apple-darwin)` cannot catch a compile break that the
arm64 build misses, because nothing in this workspace compiles differently per architecture:

```
cfg(target_arch = ...) attributes under crates/          : 0   (grep exit=1)
core::arch::x86 / std::arch::x86 / asm! / is_x86_feature : 0
KNOWN-POSITIVE CONTROL: cfg(target_os = ...)             : 64 files
```

The control is load-bearing: "zero hits" is the easiest claim in this program to pass with a dead
instrument (§3b-i), so the same matcher was pointed at `cfg(target_os` in the same tree and
returned 64 files. Every raw `target_arch` string in the tree is the identifier
`target_architecture` — a release-receipt struct field — plus one doc comment.
`target_triple_for()` is runtime string mapping, not conditional compilation.

Residual Intel risk is dependency-side only, and still runs unconditionally on `main`, integration
and every PR.

## 4. Decision, cross-audited

Panel question: (A) move the Intel job on as briefed, (B) move nothing and report saturation,
(C) leave the job set alone but cut per-job cost with a persistent `CARGO_TARGET_DIR`.

| auditor | vote |
|---|---|
| codex gpt-5.6-sol | **B** — redundant job at 84.8% indefensible; shared target dir weakens clean-build confidence for release artifacts |
| gemini 3.1-pro | **B** — at 85% utilisation waits grow exponentially; unpruned target dir → thrash, disk exhaustion, incremental-compile risk |
| kimi K3 | **C** — "B is a shrug; saturation is the symptom, cold builds are the cause" |
| internal adversarial | argues **C** — doing nothing leaves lanes waiting 85-151 min |

**2-1 for B; A got zero votes. I took B.** The minority does not carry stronger evidence, which is
the §4 test: kimi's case rests on "cargo fingerprinting is robust, persistent target dirs are
standard practice", and this repository has two measured counterexamples — `wcore-protocol` baking
`CARGO_MANIFEST_DIR` via `source_digest()` (LANE-BRIEF; produced failures "in files you never
touched"), and `ci.yml:182`, a step that exists solely to clear the *other* self-hosted runner's
surviving state. Decisively, C changes the provenance of the artifact every other lane live-tests
with, and proving it safe needs repeated warm builds across branches on a runner already at 84.8%.
Shipping it unproven is exactly the "YAML edit reported as a working change" the brief forbids.

Vote-extraction notes (each panel leg silently drops a vote if invoked wrong): codex repeats its
final block → took the LAST match; gemini needs `--skip-trust`; kimi indents and bullet-prefixes →
extracted UNANCHORED, since `^[ABC]$` would have lost a vote sitting at `  • C`.

## 5. Live proof — the job runs, and the gate still fails when it should

A YAML diff proves nothing and I changed no YAML, so what needed proving is that the *existing*
wiring genuinely executes at my base and that its guard is still falsifiable.

**Run `30523806783`, commit `9f3e1543`, branch `lane/darwin-ci-selfhosted`.** Job-level API:

```
name        : Build (aarch64-apple-darwin) [self-hosted]
conclusion  : success
runner_name : sean-mac-arm64      runner_id : 34
head_sha    : 9f3e15439364f5e3be55b51bc77ec8a7bcabeccf   <- matches my HEAD exactly
created_at  : 07:42:13Z   started_at : 07:42:16Z    queue wait 3 SECONDS
completed_at: 07:50:16Z                             run time  8.00 min
steps: 9/9 success, including the arch assertion and the arm64 verification
```

Artifact re-verified independently after `gh run download` on this Mac:

```
file -b     -> Mach-O 64-bit executable arm64
lipo -archs -> arm64
./wayland-core --version -> wayland-core 0.12.25   (rc=0)
DISCRIMINATING CONTROL: lipo -archs /bin/ls -> x86_64 arm64e
```

The control matters because "lipo said arm64" is worthless from a tool that says arm64 for
everything; it reports other and multiple architectures, and distinguishes `arm64` from `arm64e`.

**Both directions, on the rationing itself.** "Zero hosted macOS jobs ran" is an absence claim, so
it was run against a known-positive — the identical matcher, one variable (branch):

| run | branch | `grep -cE 'macos-latest\|x86_64-apple-darwin'` over job names |
|---|---|---|
| 30523806783 (mine) | `lane/darwin-ci-selfhosted` | **0** |
| 30517009690 (control) | `plan/f20-unified-audit-repair` | **2** — `CI (macos-latest)`, `Build (x86_64-apple-darwin)` |

So the gate can pass AND can fail, and the absence in my run is real rather than a dead matcher.
I also confirmed the self-hosted job **appeared by name** in my run's job list — the brief's
"a participant that never started reports a clean run" trap.

**Inherited invariant gate, re-run against my tree:** `GATE_FAILURES=0` across its 11-case truth
table (regression check — I broke none of the previous lane's invariants), and
`gate.py --self-test` → `SELF_TEST=PASS` with all five mutation arms still detected (mutual
exclusion, fork-PR safety, runner retarget, concurrency coalescing, Rosetta assertion), each with
the "naive matcher would have missed it" third assertion.

**Honest caveat on my own queue number.** My job waited 3 seconds; jobs measured earlier the same
morning waited 85-151 minutes. Queue wait here is bimodal — near-zero when the runner is free,
very long during lane fan-out. My single sample is a best case and must not be read as evidence
the queue problem is absent. The 84.8% duty cycle is the honest capacity figure, not my 3 seconds.

## 6. Instrument defect found in my own work, and repaired here (§6b-ii)

My panel evidence copy **silently produced a 0-byte `panel-kimi.txt`** while the 1907-byte source
was intact — and it was committed that way. Had I not diffed the byte counts, this lane's record
of a dissenting vote would have been an empty file, i.e. the dissent would have vanished from the
evidence while the summary claimed to have weighed it.

Repaired, not merely noted: the copy step now asserts `src_bytes == dst_bytes && != 0`, and the
assertion is proven able to fail — a deliberately emptied probe file reddens it
(`COPY-VERIFY FAIL .probe.tmp src=1907 dst=0`). Both real files re-verified at 1907 and 5198 bytes.

## 7. What remains gated, and why

| job | where it runs on a token-less `lane/**` push | why |
|---|---|---|
| `Build (aarch64-apple-darwin) [self-hosted]` | **self-hosted Mac, every push** | cheapest, uploads the arm64 binary, natively compiled and smoke-executed |
| `Build (x86_64-apple-darwin)` | hosted, opt-in | **zero unique detection value** (§3) + no capacity (§2) |
| `CI (macos-latest)` — fmt, clippy, 12.8k tests | hosted, opt-in | 26 min of a personal machine per push, and the one job whose tests touch a real login keychain; the canonical verdict should stay hermetic |

`main`, integration and every `pull_request` are untouched and keep the full hermetic hosted
matrix. `release.yml` untouched, so every shipped artifact still comes from a hosted runner.

**Still absent on a token-less lane push:** the macOS test/lint verdict and the Intel compile.
Both surface one merge hop later, at integration.

## 8. Recommended follow-ups (not done here)

1. **Throughput, not placement, is the binding constraint.** Revisit a persistent
   `CARGO_TARGET_DIR` outside the cleaned workspace with the panel's named mitigations
   (per-triple dir, pruning, an artifact-provenance check), measured as its own lane. Only after
   the warm-build duty cycle is re-measured should any second macOS job be reconsidered.
2. **The fork-PR exposure (`lane/ci-selfhosted-mac` §7.1) is still open** — public repo,
   `first_time_contributors` approval, self-hosted Windows on the `pull_request` path. Highest
   severity thing in this area and it predates both lanes. I did not widen it: the macOS job
   remains `push`-only.
3. **`ferrox-win-msvc` serves zero wayland-core jobs** while online and busy. Either it is
   intentionally dedicated to another repo — in which case capacity planning that counts three
   runners is wrong — or it is mis-scoped.

## 9. Fences and what I did NOT do

- **Shared-file fence CLEAN**: `git diff 4caaa31c -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` → 0 bytes; known-positive on my evidence dir → 11 files,
  468 insertions.
- **Zero files changed under `crates/` or `.github/`.** No workflow file was edited by this lane.
- **The runner was not registered, unregistered, relabelled, reconfigured or stopped**; read-only
  `gh api` throughout. Nothing under `/Users/seandonahoe/dev/waylandcore/actions-runner` touched.
- **No Windows host or `C:\actions-runner-*` touched**, and no Windows config read or written.
- **No cargo run on this Mac** — this lane compiled nothing locally; the only binary executed was
  the CI-produced artifact (`--version`), which is not a build.
- **No run cancelled.** One push, one run, one runner slot (~8 min) — deliberately minimal given
  the 84.8% duty cycle.
- **Not done (Sean-reserved):** no merge to `main`, no PR, no tag, no release, no issue closed,
  no `wcore-contract generate`, no secret or credential touched.
- Every number here came from `gh api` or from `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/wc`,
  `/usr/bin/env python3` by absolute path, redirected to a file and read with the Read tool.
  The push was verified by comparing `git ls-remote gh` to local `HEAD`, not by exit status.
