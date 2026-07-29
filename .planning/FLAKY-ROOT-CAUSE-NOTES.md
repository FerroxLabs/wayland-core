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

## Open questions I must not paper over

- Does nextest ever run more than one test per process here? (`--lib` is one binary; I
  will verify empirically by printing PIDs, not by asserting the model from memory.)
- Are any of the 2160 lib tests `#[ignore]`d, so a "green" is vacuous? Must read back
  `N passed` **and** `N skipped` from raw, unproxied output.
- Is the test count itself unstable (2160 vs 2170 across two serial runs)? **That gap is
  10 tests and nobody has explained it.** A moving denominator is its own finding.
