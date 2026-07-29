# CI-IMAGE — the container was the defect, and it is three packages, an init, and four flags

Lane `lane/ci-image`, base `plan/f20-unified-audit-repair` @ `0b5182ef`, HEAD `8c93cfe8`.
Running notes: `.planning/evidence/ci-image/CI-IMAGE-NOTES.md`.
Raw transcripts: `c4-probe-RESULT.txt`, `bwrap-grant-RESULT.txt`.

---

## 0. Headline

- **All 52 image-caused failures are addressed by one file.** `ci.yml` only —
  **zero lines of `crates/` touched**, shared-file fence untouched (verified against
  the merge-base SHA, not the branch name).
- **The 13 reaping failures have a named, measured mechanism**, not a class label.
  It is not parallelism, not the missing `ps`, and not a containment failure: the
  container has no reaping init, so a killed descendant stays a **zombie**, and all
  13 probes count a zombie as alive. Containment genuinely succeeds; the probe
  cannot tell a corpse from a live process.
- **The bubblewrap blocker the previous lane recorded is refuted.** It diagnosed
  the bind-mounted `/work`. Running the identical bwrap argv against a
  container-internal directory fails **identically**, so the mount was never the
  variable. The real blocker is Docker's masked `/proc`, and the flag that closes
  it was never tried.
- **`WAYLAND_ALLOW_NO_SANDBOX=1` is not set anywhere, and nothing is skipped.** The
  20 bwrap tests now *run*. The skip-escalation env var is armed so that if the
  grant ever regresses, the leg goes **red** rather than qualifying out quietly.
- **68 failed → 2** in the real CI run (`30410531297`). Graded by name, **65 of the
  original 68 now pass**; the 3 that do not are the one Sean-reserved true red and
  two pre-classified container-timing flakes. Nothing was silently skipped:
  `50 skipped` both before and after, and `12838` tests ran where `12820` did.
- **Four defects in my own instruments, repaired in this lane**, three of which
  would each have produced a wrong headline.

---

## 1. What I changed

One file: `.github/workflows/ci.yml`, `ci-linux` job. `+107 / -11`.

| change | closes | why it is right |
|---|---|---|
| `python3`, `procps` into the inline Dockerfile | 29 | `portability_hostile_corpus.rs:119` `.expect()`s `python3`; `orphan.rs:321` is the ONLY site that shells out to `ps` |
| `--init` on `DOCKER_RUN` | 13 | gives the container a reaping PID 1, as every real Linux host has |
| `bubblewrap` + 4 grants, on the **test step only** | 20 | package alone is necessary and not sufficient — measured |
| `WCORE_REQUIRE_ENFORCING_SANDBOX=1` | — | converts a future silent skip into a hard failure |

Every one of the four grant flags is commented in `ci.yml` with the exact refusal
it closes, so the next reader cannot copy them without the measurement.

---

## 2. The reaping 13 — mechanism NAMED

The brief asked me not to lump these in, and they did not belong lumped in.

`DOCKER_RUN` carried no `--init`, so **PID 1 inside the container is the test
command itself**. Nothing in `crates/` sets `PR_SET_CHILD_SUBREAPER` (grepped; the
only `prctl` calls are credential drops in `wcore-eval-scenarios/src/process_tree.rs`),
so an orphaned descendant reparents to PID 1. Rust's `Child::wait()` issues
`waitpid(<specific pid>)` — never `wait(-1)` — so PID 1 **cannot** incidentally reap
an adopted orphan. The corpse remains a zombie indefinitely.

Every one of the 13 probes is satisfied by a zombie:

| probe site | shape | why a zombie satisfies it | n |
|---|---|---|---|
| `runner_contracts.rs:125` | `kill(pid,0)==0 \|\| errno != ESRCH` | `kill` returns 0 for state `Z` | 7 |
| `pty_capture.rs:783` | identical | same | 2 |
| `wcore-sandbox/tests/process_capture.rs:12` | `Path::new("/proc/{pid}").exists()` | a zombie has a `/proc` entry | 2 |
| `wcore-swarm/src/worktree_tests/linux.rs:629` | `/proc/{pid}` | same | 2 |

**7 + 2 + 2 + 2 = 13, with nothing left over.** That exact accounting is why I am
willing to call this the mechanism rather than a hypothesis.

Measured four ways on `hetzner-dsm` (Ubuntu 24.04, Docker 29.2.1 — a near-exact
runner match), with `zombie-probe.c` reproducing `process_exists` verbatim:

| arm | PID 1 | probe says | `/proc` state | verdict |
|---|---|---|---|---|
| native | `systemd` | GONE | `-` | PASS |
| container, **CI's exact flags** | the test command | **ALIVE** | **`Z`** | **FAIL** |
| container + `--init` | `docker-init` | GONE | `-` | PASS |
| container + `--init`, descendant **genuinely alive** | `docker-init` | ALIVE | `S` | **FAIL** |

The fourth arm is the control and it is the one that matters: **`--init` does not
make these tests unfailable.** A descendant that really survives still reds them.

This explains every prior observation without further assumption — native passes
(systemd reaps), 96-core native-parallel passes (PID 1 identity is a container
property, not a concurrency one), and it is not the missing `ps` (no probe shells out).

**A second defect here is NOT mine and is reported, not carried:** these probes
conflate a corpse with a live process. `--init` removes the trigger, not the flaw.
On any host without a reaping init they will mis-fire again. The correct probe
reads `/proc/<pid>/stat` field 3 and excludes `Z`. That is test-code owned by
other lanes; I did not touch it.

---

## 3. The bubblewrap 20 — decision and justification

### 3a. The obvious answer was already disproved, and so is the disproof's diagnosis

The previous lane established that installing bubblewrap is not enough. That
stands and I reproduced it. But its explanation of *why the grant still was not
enough* — mount propagation on the bind-mounted `/work` — is **refuted**:

| case (engine's argv from `bwrap.rs:212-349`) | result |
|---|---|
| no grant, workspace = bind-mounted `/work` | `No permissions to create new namespace` |
| grant, workspace = bind-mounted `/work` | `Can't mount proc on /newroot/proc` |
| grant, workspace = **container-internal dir** | **identical failure** |

Bind-mounted and container-internal fail the same way, so the bind mount was never
the variable. The blocker is that **Docker masks paths under `/proc`, and those
masks are locked mounts that refuse bwrap's `--proc`.** The earlier namespace probe
passed only because it ran `bwrap --ro-bind / / --dev /dev true` — **no `--proc`**.
That is the same "probe answers an easier question" defect that lane itself
flagged about `is_available()`, one level up.

### 3b. The minimal grant, each flag tied to the refusal it closes

```
--cap-add SYS_ADMIN
--security-opt seccomp=unconfined
--security-opt apparmor=unconfined
--security-opt systempaths=unconfined
```

| remove this flag | and you get back |
|---|---|
| `apparmor=unconfined` | `bwrap: No permissions to create new namespace` (Ubuntu 24.04 sets `kernel.apparmor_restrict_unprivileged_userns=1`) |
| `seccomp=unconfined` | `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` |
| `SYS_ADMIN` | `bwrap: Failed to make / slave: Permission denied` |
| `systempaths=unconfined` | `bwrap: Can't mount proc on /newroot/proc: Operation not permitted` |

### 3c. The decision: grant it, scoped to the test step

**Not a skip.** With bubblewrap installed in **both** arms, so the package is
isolated from the grant, on the exact image `ci.yml` now builds:

| crate | without grant | with grant |
|---|---|---|
| `wcore-sandbox` | 100 run, 86 passed, **14 failed** | 100 run, **100 passed, 0 failed** |
| `wcore-swarm` + `wcore-tools` | 1344 run, 1330 passed, **14 failed** | 1344 run, **1344 passed, 0 failed** |

**Zero regressions in either crate** — including the `fail_closed` and
`hard_containment` cases, which are the ones most sensitive to sandbox posture.

**Why granting is defensible.** This container is a workaround for a GHA
runner-agent crash, **not a boundary against the code under test** — the workflow
already runs arbitrary repo code directly on the ephemeral runner in the macOS and
Windows legs. The grant moves the Linux container *toward* a normal Linux host,
which is the environment these tests are written for; the container's extra
restrictions are the anomaly. This is the same argument as `--init`, and I applied
it consistently: **each way the container deviates from a real host is the defect,
and should be closed rather than worked around in test code.**

**Why not a skip.** A skip would have meant editing 20 tests across 5 crates —
20 fresh opportunities to create a test that proves nothing, in files owned by
other lanes, immediately after this program measured a "counted" skip that counted
nothing. And most of the 20 are swarm dispatch/worker tests whose subject is not
bwrap at all; they merely need *some* live sandbox to start a worker. Skipping
them would have withdrawn real coverage to work around an environment defect.

**Cross-audit (§4), and the dissent.** codex `A`, gemini `A`, kimi `C`
(separate privileged job, on scope-discipline grounds: "grant scope should track
need scope, not suite scope… the next person copies the line without the
measurement"). Majority `A`; **I adopted a hybrid closer to kimi's position than
to the majority's**, because its objection was the only one with a concrete
failure mode attached:

- the grant is a **second variable used by exactly one step**, not folded into
  `DOCKER_RUN` — fmt, clippy, build, release-smoke and audit keep the hardened
  posture (asserted mechanically: 1 step grants, 7 stay hardened);
- every flag carries its measured refusal in-line, and an explicit
  "do NOT copy these flags to another step without re-measuring";
- kimi's "steal B's probe anyway" is adopted: `WCORE_REQUIRE_ENFORCING_SANDBOX=1`
  is armed, so a regression reds the leg instead of skipping.

I rejected kimi's full option C on cost: a second full workspace compile in a job
that already runs 12,820 tests under a 90-minute timeout, to isolate a container
that is not a security boundary.

**Internal adversarial pass (arguing against the consensus).** The strongest case
against granting is not security, it is *measurement fidelity*: the grant changes
the kernel surface for all 12,820 tests, so a test could pass for a different
reason — this program's most-repeated defect class. That is exactly why I ran the
without/with comparison on both affected crates rather than only checking that the
20 turned green. 1,444 tests measured across both arms, zero regressions. The
objection is answered by measurement, not by argument. The residual risk is the
~11,000 tests I did not compare arm-to-arm; the real CI run below is what bounds it.

---

## 4. The real CI run — id `30410531297`

`gh run list -R FerroxLabs/wayland-core --branch lane/ci-image`
Job `CI (linux-containerized)` = `90445351454`, head SHA `8c93cfe8`.
Log pulled with `gh api /repos/.../actions/jobs/90445351454/logs` (the `--log`
path is intercepted by `rtk` and returns rc=1 without downloading).

### The number

```
attempt 1  Summary [ 480.990s] 12838 tests run: 12836 passed (2 slow, 1 flaky, 1 leaky), 2 failed, 50 skipped
attempt 2  Summary [ 473.056s] 12838 tests run: 12835 passed (1 slow, 1 leaky),          3 failed, 50 skipped
```

**68 failed → 2.** The job's conclusion is **failure**, and that is the honest
outcome: two real failures remain and I am reporting them red rather than
engineering a green.

### The original 68, graded BY NAME (not by crate total)

| grade | n |
|---|---|
| **PASS** | **65** |
| FAIL | 3 |

The three, unioned across both attempts (attempt 1 had 2; attempt 2 added one more):

| test | class | mine? |
|---|---|---|
| `wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte` | **R1 — the one true red** | **No.** Sean-reserved: needs `wcore-contract generate`, forbidden by brief §0. Already a fenced seam request in `RED-68-TRIAGE.md`. |
| `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` | C5 container timing | No. Pre-classified C5; passes serially on the build host. |
| `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` | C5 container timing | No. Failed in attempt 2 only — flaky, not deterministic. |

The arithmetic closes exactly: 68 = C1 23 + C3 20 + C4 13 + C2 6 + C5 3 + K1 1 +
S1 1 + R1 1. Failing now = R1 (1) + two of C5 (2) = 3. So **every member of C1,
C2, C3 and C4 — all 62 — now passes**, plus K1, S1 and the third C5.

### The falsifiable prediction I stated before the run

I predicted the 13 reaping tests would pass with `--init`, and said I would call
the mechanism wrong if they did not. Graded individually by name:

```
C4 descendant reaping (n=13) -> {'PASS': 13}
```

**13/13. The prediction held**, so the zombie/no-reaping-init mechanism stands as
the named cause rather than a hypothesis.

### That nothing was silently skipped — the check that matters

A cluster of failures turning into a cluster of *skips* would look identical in a
pass count. It did not happen:

- **`50 skipped` in both attempts — the identical number the pre-fix run reported.**
  Not one test converted from failing to skipped.
- **`12838` tests run, up from `12820`.** More tests execute, not fewer.
- **Zero `WCORE_SANDBOX_SKIP` records**, with `WCORE_REQUIRE_ENFORCING_SANDBOX=1`
  armed — so a skip would have been a hard failure, not a silent pass. The step
  printed: `no sandbox-qualified skips recorded — the enforcing sandbox was live
  for this leg`. **That is the load-bearing proof the bwrap grant works in real
  CI**, and it is a positive signal rather than an absence.
- The image really did install them:
  `Setting up python3 (3.11.2-1+b1)`, `Setting up procps (2:4.0.2-3)`,
  `Setting up bubblewrap (0.8.0-2+deb12u1)`.

### Other legs

`Eval acceptance gate (Linux)`, `Browser live e2e`, and all four `Build` jobs:
**success**. The self-hosted Windows leg (`CI (Array)`) failed with
`12487 tests run: 12411 passed, 76 failed, 116 skipped` — **not this lane**. Every
one of my seven diff hunks lands between ci.yml lines 283 and 484, and the
`ci-linux` job spans 268–501, so the Windows job is untouched by this change.
macOS was still queued on shared runners when I finished; it is likewise untouched.

---

## 5. Defects in my own instruments, repaired here (§6b-ii)

Four, all repaired in this lane rather than written up and carried. Three of the
four would each have produced a wrong headline.

1. **My CI grader indexed 2 status lines and called the other 66 "ABSENT".** I cut
   the log at the `Summary` line, but `final-status-level = "all"` makes nextest
   print the full per-test status list *after* that line, and `status-level = "fail"`
   means passes appear nowhere else. So my first grading of the 68 returned
   `ABSENT: 66, FAIL: 2` — which, reported as-is, would have claimed I could not
   verify 66 of the tests I had just fixed. Repaired to index the whole log; the
   repaired instrument indexes **12,838 distinct tests, exactly matching the run's
   own `Summary` count** — an independent third oracle confirming the index is
   complete rather than merely larger. That agreement is what makes the 65/3
   grading trustworthy; without it "65 passed" would rest on absence again.

2. **I read "absent from the Windows failure list" as "passed on Windows".** For a
   `#[cfg(target_os = "linux")]` test, absence means it **does not exist** there. I
   used that shape to argue the bwrap tests had native coverage elsewhere, which
   would have overstated the case for skipping them. Repaired:
   `classify-windows-status.py` consults the cfg gate as an independent second
   oracle and returns a third state, `NOT_PRESENT`, instead of folding it into
   `PASSED`. Self-test **3 passed, 0 failed**; A3 reports
   `repaired=NOT_PRESENT_ON_WINDOWS  old=PASSED_ON_WINDOWS` — i.e. the old shape
   provably misclassified.
3. **My first bwrap matrix put `--ro-bind / /` after `--proc`/`--dev`**, overmounting
   them, and produced a spurious `cannot create /dev/null` that was my harness's
   error and not the kernel's. It did not change the B-vs-C conclusion (both failed
   identically at the proc mount), but an unexplained error in a capability matrix
   is exactly how a wrong recipe gets adopted. Recorded rather than dropped.
4. **A YAML defect my pre-push validation caught before it shipped.** My step name
   contained `expected: none`; an unquoted colon-space is invalid YAML and would
   have made the **entire workflow unparseable** — producing a run that measured
   nothing while looking like a lane that had done its job. The validator now also
   asserts the grant reaches exactly one step and the other seven keep the hardened
   `DOCKER_RUN`.

---

## 6. What I did NOT do — stated plainly

- **Did not fix the probe defect behind the 13.** `--init` removes the trigger; the
  probes still cannot distinguish a zombie from a live process, and will mis-fire on
  any host without a reaping init. That is test code in four crates owned by other
  lanes. Named precisely in §2 with the exact fix.
- **Did not touch a single line of `crates/`.** No test weakened, `#[ignore]`d,
  re-gated, deleted, or re-timed. `WAYLAND_ALLOW_NO_SANDBOX` is set nowhere.
- **Did not arm-to-arm compare the grant across the whole workspace** — only the two
  crates that carry the 20 (1,444 tests). The rest is bounded by the CI run, not by
  a build-host measurement.
- **Did not address the one true red**, the contract-digest guard (R1). It is
  Sean-reserved and already written as a fenced seam request in `RED-68-TRIAGE.md`.
- **Did not re-push to retrigger.** One push, one run, polled.
- Did not merge, open a PR, tag, close an issue, or touch `main`.
