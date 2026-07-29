# NOTES — lane `ci-selfhosted-mac` (working log, appended as measured)

Branch `lane/ci-selfhosted-mac`, merge-base `75babf32`. Committed inside the first 15 minutes
per LANE-BRIEF §6b-i, and re-committed after every measurement.

---

## 0. Plan (written before measuring, so it can be graded against what I actually found)

Task: decide which of the three macOS jobs move to the new `sean-mac-arm64` self-hosted runner,
restore lane-branch macOS coverage to the degree the new capacity supports, prove it live with a
counterfactual, and state the hermeticity cost.

Working hypothesis to test, NOT to assume:
1. A single self-hosted runner has **lower throughput** than the 5-wide hosted pool. If true,
   "move everything" is wrong and the win is *latency at low demand*, not capacity.
2. The right split routes **lane** macOS demand to self-hosted and leaves **main / integration /
   PR** on the hosted pool.
3. The `[ci-darwin]` rationing must stay for whatever remains hosted.

Steps: (a) measure real job durations from the API; (b) compute serial arithmetic both ways;
(c) check fork-PR exposure; (d) implement; (e) live-dispatch and prove `runner_name`; (f) prove
the artifact is genuinely arm64; (g) counterfactual; (h) prove Windows untouched.

---

## 1. Facts established at start

### 1.1 The runner (`gh api repos/FerroxLabs/wayland-core/actions/runners`)

```
id 34  sean-mac-arm64  macOS  status=online  busy=false  v2.336.0
labels: self-hosted:read-only  macOS:read-only  ARM64:read-only
```

All three labels are `read-only` = **self-detected by the runner**, not hand-written. Per the
brief this is the distinction that was earned the hard way (the first install carried `ARM64` as
a `custom` label while the runtime self-reported `X64` — an x64 package under Rosetta). This
install is clean. Contrast, same call, the two Windows runners:

```
id 22  ferrox-win-msvc  Windows  online  busy=true   labels: self-hosted:ro Windows:ro X64:ro msvc:CUSTOM
id 21  SEANDESKTOP      Windows  online  busy=true   labels: self-hosted:ro Windows:ro X64:ro msvc:CUSTOM
```

`msvc` is `custom` on both — that one is a *tag*, not an architecture claim, so it is benign. The
rule I am carrying forward: **a `custom` architecture label is an unproven claim.** Neither
Windows runner asserts its arch via a custom label, so neither is suspect.

### 1.2 FINDING (NEW, HIGH) — the repository is PUBLIC, and self-hosted runners already serve `pull_request`

```
$ gh api repos/FerroxLabs/wayland-core --jq '{private,visibility,allow_forking}'
{"allow_forking":true,"private":false,"visibility":"public"}
```

`ci.yml` triggers on `pull_request: branches: [main]`, and the `ci` job's matrix already contains
`["self-hosted","Windows","X64","msvc"]`. So **fork-PR code paths already reach Sean's self-hosted
Windows boxes**, and naively adding `[self-hosted, macOS, ARM64]` to that same matrix would extend
the same exposure to his personal Mac — the machine holding his login keychain, SSH keys and every
worktree in this program.

This is a pre-existing exposure I did **not** create and will **not** silently widen. Design
consequence, adopted before writing any YAML: **the self-hosted macOS runner is used on `lane/**`
pushes only — never on `pull_request`, never on `main`.** Verifying the fork-PR approval policy
next; even with approval-gating on, "one careless Approve" is not a boundary I want in front of a
personal machine.

### 1.3 The runner is on THIS Mac, inside the forbidden checkout

```
$ pgrep -lf Runner.Listener
2263 /Users/seandonahoe/dev/waylandcore/actions-runner/bin/Runner.Listener run --startuptype service
$ uname -m -> arm64 ;  macOS 26.3 (25D125)
```

Two consequences:

- The runner's work tree lives under `/Users/seandonahoe/dev/waylandcore` — the heavily-dirty
  checkout LANE-BRIEF §0 forbids me to touch. I will not touch it; noting only that runner jobs
  land on the same volume.
- **LANE-BRIEF §0's "never run cargo on the Mac" applies in spirit here.** The rule exists because
  Mac builds are slow and were causing real problems. Routing CI to this runner does not violate
  the letter (I am not invoking cargo), but it hands the same cost to the same machine via a
  different door. That is exactly the capacity question I was asked to answer honestly, and it
  argues against moving the heavy job.

### 1.4 What the three jobs are FOR (read from ci.yml, not assumed)

| job | what it actually does | why it exists |
|---|---|---|
| `CI (macos-latest)` | fmt, clippy, **full 12.8k-test nextest**, contract check, release smoke, eval gate, `cargo audit` | the Darwin **correctness verdict** |
| `Build (aarch64-apple-darwin)` | `cargo build --release -p wcore-cli --target aarch64…` + **uploads the arm64 artifact** | the **artifact producer** a lane live-tests with; uploads regardless of test outcome (ci.yml:646-654 records the 2-1 cross-audit that kept it for exactly this reason) |
| `Build (x86_64-apple-darwin)` | same, x86_64 target, uploads Intel artifact | Intel **compile-regression** check + Intel release binary |

---

## 2. Open / next

- [ ] real durations for the three jobs from the API → serial arithmetic
- [ ] current hosted macOS queue wait (post-rationing; the 4-5h figures are pre-fix and must not
      be reused as the comparison baseline)
- [ ] fork-PR approval policy
- [ ] implement, dispatch, prove `runner_name`, prove arm64, counterfactual, Windows unaffected

---

## 3. Measurements that decided the design

### 3.1 Executed macOS job durations (n=14 executed; cancelled EXCLUDED)

From `gh api .../runs/<id>/jobs` over a 100-run `ci.yml` window, filtered to
`conclusion in (success,failure)` — the prior lane's measured instrument defect was counting
cancelled jobs whose enqueue→cancel span reaches 368 min, which reported 61 concurrent where the
truth was 5. I filtered the same way.

| job | n | median run | max run | median QUEUE wait |
|---|---|---|---|---|
| `CI (macos-latest)` | 7 | **26.2 min** | 32.7 | 56.2 min |
| `Build (aarch64-apple-darwin)` | 4 | **16.5 min** | 23.5 | 17.3 min |
| `Build (x86_64-apple-darwin)` | 3 | **16.5 min** | 23.4 | 89.5 min |

### 3.2 THE SERIALISATION ARITHMETIC (the question I was asked to answer, not assume)

A self-hosted runner executes **one job at a time**.

- move all three  -> 26.2 + 16.5 + 16.5 = **59.2 min serial** -> **~3.0 macOS jobs/hr**
- hosted pool measured (prior lane, independently)            -> **11.25 macOS jobs/hr**

**Moving everything is a 3.7x throughput DOWNGRADE.** Self-hosted is NOT strictly better. Its
advantage is *latency while idle*, not capacity.

Live census 2026-07-29T06:41:37Z on the hosted pool: **39 macOS jobs queued / 3 in_progress**.
So the runner's real value is that it has no queue at all.

Demand check for moving ONE job: lane traffic measured at 234 macOS jobs / 3-per-push / 10.84 h
= **~7.2 lane pushes/hr** at peak fan-out. 7.2 x 16.5 min = **119 min of work per 60 min of
capacity — 2x oversubscribed even for one job.** That is why the job carries a per-ref
`concurrency` group with `cancel-in-progress: true`: a superseded commit's binary is worthless,
and coalescing collapses a burst to one build. Without it I would have rebuilt the hosted pool's
pathology on Sean's laptop.

### 3.3 Decision

| job | destination | why |
|---|---|---|
| `Build (aarch64-apple-darwin)` | **SELF-HOSTED** on token-less `lane/**` pushes | cheap, native, the only arm64 artifact producer, uploads regardless of test outcome |
| `CI (macos-latest)` | **stays HOSTED**, opt-in on lanes | 26.2 min of a personal laptop; and it is the job whose 12.8k tests (keychain, fsevents, sandbox, reaping) are most corrupted by a non-hermetic desktop |
| `Build (x86_64-apple-darwin)` | **stays HOSTED**, opt-in on lanes | compile-check only, Intel, lowest urgency; adding it would serialise a second 16.5 min behind the first |

### 3.4 Correction to an inherited assumption — clippy red is NOT macOS-specific

`CI (macos-latest)` is 7/7 `failure` in the window, failing at **Clippy**. I nearly recorded that
as evidence of macOS-only value. It is not: on the same run `30421332107`, `CI (linux-containerized)`
ALSO fails at Clippy (step 8) and `CI (Array)` (Windows) fails too. It is a repo-wide clippy red.
Stated because the tempting version of this claim would have overstated my lane's case.
On that same run `Build (aarch64-apple-darwin)` was **green** — the build leg is reliable while
the verdict leg is red, which is exactly why the build leg is the one worth moving.

### 3.5 No contention for the runner

No workflow targets `[self-hosted, macOS, ARM64]` today. Absence proven with a live instrument:
the same grep finds **15** self-hosted Windows `runs-on` matches (nightly-windows-soak.yml x3,
ci.yml matrix), and per-file known-positives of 13 and 10. The three macOS+self-hosted "hits" are
comment prose and the Windows matrix line. So I am the runner's first consumer.

## 4. Instrument defects found in MY OWN work, and repaired here (LANE-BRIEF 6b-ii)

Not written up and left — repaired in this lane, each with the third assertion.

1. **`gate.py --self-test` reported `rc=0` on a CRASHED run.** I invoked it as
   `python3 gate.py ... | tee file; echo $?` — **the pipe stole the exit status**, the exact
   defect LANE-BRIEF 3.2 names, committed by me while implementing a gate against that class.
   Repaired: status is now read with no pipe (`cmd > file; RC=$?`).
2. **An evaluator exception aborted the remaining self-test arms.** Arm 2 raised on a `!=`
   clause; arms 3, 4 and 5 never ran while the suite still printed plausible output. Repaired:
   `!=` is modelled, an unmodelled clause is a typed `Unparsed` that becomes a gate FAILURE
   (never coerced to False, which would report the safe answer for an expression the gate never
   understood), and each arm is exception-isolated.
3. **Two arms were undetected — genuine gate weakness, not mutation noise.**
   - *fork safety*: my only `pull_request` case used `ref='main'`, so `startsWith(ref,'lane/')`
     excluded it incidentally and deleting the `event_name == 'push'` guard changed nothing the
     gate could see. Added a `pull_request` case with a lane-shaped ref, so the push-only guard is
     now the only thing producing the safe answer.
   - *Rosetta assertion*: I tested `"exit 1" in step_body`, but the step has a SECOND `exit 1`
     for `RUNNER_ARCH`, so gutting the uname branch still satisfied it. Replaced with
     `fatal_on_arch_mismatch()`, which scopes the search to the uname branch itself.
4. **My own success message lied on failure.** The "naive matcher would have missed it" line
   printed next to `GATE_FAILURES=0` arms. Now only printed when the gate actually caught it.

Third assertion, present on every arm: the **naive matcher** (`self-hosted` + `ARM64` + job name
all appear in the file) **PASSES all five mutations** while the real gate fails them. Without that
comparison the self-test would pass on a broken gate too.

Result: `SELF_TEST=PASS`, rc=0, 5/5 arms detected.

## 5. Still open

- [ ] live dispatch: prove a job executed on `sean-mac-arm64` via job-level `runner_name`
- [ ] prove the artifact is genuinely arm64 (`file`/`lipo`, and execute it)
- [ ] counterfactual: same work under pre-change config queues behind the hosted pool
- [ ] Windows runners unaffected
