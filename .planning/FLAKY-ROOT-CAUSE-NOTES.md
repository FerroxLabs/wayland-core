# FLAKY-ROOT-CAUSE — working notes

Lane: `flaky-root-cause`. Branch `lane/flaky-root-cause`, base `75babf32`.
Started 2026-07-29. **Append after every measurement — never batch to the end.**

Target: `cargo nextest run -p wcore-agent --lib` is nondeterministic.
Reported: run A 22 failures, run B 18 failures, differing sets both directions;
`--test-threads=1` measured 2160/0 and 2170/0.

---

## Environment of record

- Build/test host: `hetzner-dsm` (`Ubuntu-2404-noble-amd64-base`), **`nproc` = 96**.
  nextest default `--test-threads` = 96. Flake rate is expected to depend on this.
- Disk at start: `/dev/md2 1.8T, 980G used, 686G free (59%)` — headroom fine.
- Load at start: `load average: 8.36, 4.43, 3.50`, 5 other lanes live. **Contention is a
  confound I must control for** (LANE-BRIEF §6: EMFILE cluster in `wcore-skills` etc.).
- Mac side is measurement-only; no cargo on the Mac.

## Instrument discipline (LANE-BRIEF §3b)

`rtk` rewrites `cargo`/`grep`/`git` output and **strips `0 ignored` / `0 filtered out`**.
Every load-bearing number here comes from `/usr/bin/...` absolute paths or a raw captured
file. Every absence claim gets a known-positive in the same invocation plus its query text.

---

## Hypothesis under test — and why the received one is suspect

The program's label is **`CLASS-ENV-01`: "process-wide environment mutation in parallel
tests"**. The brief is explicit that this has never been proven.

**I think it is wrong as stated, for a structural reason.** `cargo nextest` executes
**one process per test**. That is its core execution model, not a tunable. Process-wide
env mutation therefore **cannot** leak from test A to test B under nextest — each test
starts from a fresh copy of the runner's environment and its `set_var` dies with its
process. `--test-threads=1` under nextest does not serialise threads inside one binary;
it serialises *processes*.

So the fact that `--test-threads=1` fixes it does **not** implicate in-process env state.
It implicates **something shared outside the process**. Candidates, in the order I will
test them:

1. **Shared filesystem paths that are not per-test temp dirs.** Already found by
   inspection (unproxied `/usr/bin/grep` over `crates/wcore-agent/src`, 165 hits for
   `set_var|remove_var`), including literal hardcoded absolute paths:
   `crates/wcore-agent/src/plugins/sig_verifier.rs:257` → `/tmp/wl-trust-test`,
   `:266` → `/tmp/explicit-trust`. Two concurrent processes touching one fixed path is a
   real inter-process race in a way `set_var` is not.
2. **Fallback to the real `$HOME`.** Many sites set `WAYLAND_HOME` / `CODEX_HOME` /
   `GROK_HOME` to a tempdir and then *restore or remove* them. Under nextest the restore
   is pointless, but the failure mode is: a test that does **not** set it reads the real
   home and races other processes in the same real directory.
3. **Lease / lock files** (the brief's "lease contention"), ports, and any global
   singleton persisted to disk.
4. **Resource exhaustion under 96-way concurrency** — fd/inotify limits, thread limits.
   This is a *different* mechanism that would also be cured by `--test-threads=1` and
   would also produce shifting failure sets. It must be separated from (1)-(3), not
   conflated with them. §6 of the brief records exactly this for `wcore-skills`.
5. **Several mechanisms at once.** The brief warns that one tidy cause is the answer we
   *want*. (1)-(4) are not mutually exclusive and the shifting failure set is weak
   evidence for more than one.

Note that (4) can be induced by *other lanes* rather than by this suite, which is why
every headline number must be taken with the machine's concurrent load recorded.

## What would count as proof (writing this down before measuring, deliberately)

- **Not** "parallel red, serial green" — that is the starting correlation, not the finding.
- Required: name the specific shared resource; show the specific tests that contend for
  it; **force** the interleaving and reproduce the failure on demand; then **remove the
  sharing** and show the same forced interleaving is clean.
- Then repetition: ≥20 runs at default concurrency post-fix with the full distribution,
  and the **same harness against the pre-fix tree** showing the flakes. A fix never shown
  to move the distribution is not a fix.

## Status log

- [x] Worktree verified: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-flaky-root-cause`,
      HEAD `75babf329235484684ecee3a65973b0c197840c1`, branch `lane/flaky-root-cause`.
- [x] hetzner reachable, 96 cores recorded.
- [x] Notes committed (this file) — 15-minute rule.
- [ ] hetzner worktree `hz/flaky-root-cause` created at base.
- [ ] Baseline: N repetitions at default concurrency, full failure distribution captured raw.
- [ ] Failing-set intersection/union analysis across repetitions.
- [ ] Shared-resource identification per failing test.
- [ ] Forced-interleaving reproduction.
- [ ] Fix + isolation.
- [ ] ≥20-run post-fix distribution.
- [ ] Pre-fix counterfactual with the identical harness.

---

## MEASUREMENT 1 — baseline at base commit, default concurrency. THE FLAKE DID NOT REPRODUCE.

hetzner `/root/wayland-flaky` @ `2cfbf972` (= base `75babf32` + my notes commit; no code
delta). Harness `/root/flaky-eviden/runharness.sh`, raw `/root/.cargo/bin/cargo`, no `rtk`.

```
run=1 rc=0 load=[15.26 8.67 5.29] Summary [31.402s] 2172 tests run: 2172 passed (2 slow), 3 skipped
run=2 rc=0 load=[15.67 9.79 5.81] Summary [31.398s] 2172 tests run: 2172 passed (2 slow), 3 skipped
run=3 rc=0 load=[12.83 9.68 5.90] Summary [31.386s] 2172 tests run: 2172 passed (2 slow), 3 skipped
run=4 rc=0 load=[13.57 10.49 6.33] Summary [31.378s] 2172 tests run: 2172 passed (2 slow), 3 skipped
run=5 rc=0 ... 2172 passed, 3 skipped
```

**5/5 clean, 0 failures, at `test-threads = num-cpus` = 96.** Denominator stable at 2172
(`nextest list` = 2172, so `no-tests`/vacuity is not in play; 3 skipped are real `#[ignore]`s).
Pass-set diff between run 1 and run 5 shows only *ordering* differences, not membership.

**This is a real result and it reframes the whole task.** The suite is not unconditionally
flaky at default concurrency. Something else was true when the 22/18 failures were measured.
Differences I must now separate, before I can claim any mechanism:

- **(a) Tree.** "Unchanged tree" in the brief may mean the integration branch
  `plan/f20-unified-audit-repair` as it stood, not base `75babf32`. Must check.
- **(b) Machine load.** My runs sat at load ~13-15 on 96 cores — nearly idle. The reported
  flakes came from a window with five lanes compiling. LANE-BRIEF §6 already records that a
  contended full-workspace run is not a measurement.
- **(c) Invocation.** The default profile has **`retries = 1`**, so a merely-transient failure
  is absorbed and reported as `flaky`, not `failed`. 22 *failures* under this profile means 22
  tests failed **twice each** — or the measuring lane did not use this profile.

### Sub-finding: the timing distribution is not starvation-shaped

Only **6** of 2172 tests exceed 5s and only **4** exceed 10s. The long poles are
30.041s / 30.041s / 20.042s / 15.040s — round numbers, i.e. **timeouts, not CPU work**. The
two 30s ones are `tool_backends::gemini_vision::tests::vision_send_error_message_omits_api_key`
and `tool_backends::image_gen::tests::gemini_imagen_send_error_omits_api_key`, which POST to
**`192.0.2.1:9` (TEST-NET-1, RFC 5737, deliberately non-routable)** and wait for the connect to
fail. So they are not internet-dependent, but they *are* wall-clock-bound and they sit against a
`slow-timeout = 30s, terminate-after = 2` → **60s hard kill**.

A mass CPU-starvation story would need a big cohort near the kill line. There isn't one. That
argues against "everything got slow and got killed" as the *general* explanation, while leaving
it wide open for the handful of wall-clock-bound tests.

### Instrument defect found and recorded (LANE-BRIEF §6b-ii — must repair, not just note)

`cargo nextest list` on this host prints:
`warning: in config file .config/nextest.toml, ignoring unknown configuration key:
profile.default.no-tests`

The repo's `no-tests = "fail"` anti-vacuity guard — the one documented at length as closing the
"exits 0 having certified nothing" family — is **INERT on cargo-nextest 0.9.137** (installed
here). The comment block above it even predicts a 0.9.138 warning about `env`; the key itself is
unsupported at 0.9.137. This does not affect my numbers (I read the executed count back
directly, 2172, and cross-checked against `nextest list` = 2172) but it is a live self-passing
gate in the repo and I must report it.

## Open questions I must not paper over

- Does nextest ever run more than one test per process here? (`--lib` is one binary; I
  will verify empirically by printing PIDs, not by asserting the model from memory.)
- Are any of the 2160 lib tests `#[ignore]`d, so a "green" is vacuous? Must read back
  `N passed` **and** `N skipped` from raw, unproxied output.
- Is the test count itself unstable (2160 vs 2170 across two serial runs)? **That gap is
  10 tests and nobody has explained it.** A moving denominator is its own finding.
