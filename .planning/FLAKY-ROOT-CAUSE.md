---
lane: flaky-root-cause
root-cause: >-
  EMFILE exhaustion of RLIMIT_NOFILE in the cargo-nextest RUNNER process's
  spawn path. nextest is process-per-test and holds ~2.9 pipe fds per
  CONCURRENTLY RUNNING test; with test-threads = "num-cpus" peak demand is
  ~3 x nproc. When that crosses the soft fd limit, fork/exec of the test
  binary fails with "Too many open files (os error 24)" and nextest reports
  the test as "exec failed" — counted alongside real failures, hitting a
  different set of tests every run, and vanishing under --test-threads=1.
  The test process never starts, so no test-level shared state is involved.
  NOT process-wide env mutation (CLASS-ENV-01), which is structurally
  impossible under a process-per-test runner and is falsified below.
mechanism-proven: yes — causally, in both directions, with zero code change
runs-clean-of-total: >-
  default concurrency pre-fix 20/20 clean; default concurrency post-fix 20/20
  clean; --test-threads=384 pre-fix 10/20 clean (10 dirty, 13-81 exec-failures
  each); --test-threads=384 post-fix 20/20 clean
serialised-tests: none — no #[serial], no test-group, no test-threads cap; concurrency untouched
new-finding: >-
  (1) CLASS-ENV-01 is the wrong label for every nextest-observed flake in this
  repo and its wcore-skills row is a resource-exhaustion finding filed under an
  env-mutation heading; (2) the repo's `no-tests = "fail"` anti-vacuity guard is
  INERT on the installed cargo-nextest 0.9.137, which rejects the key as unknown;
  (3) a second, distinct per-user resource (fs.inotify.max_user_instances) is
  reachable from 3 wcore-agent lib tests and shares EMFILE's errno, which is how
  the two got conflated; (4) I could NOT reproduce the reported 22/18 failures at
  default concurrency on this tree in 37 attempts.
fence-exposure: none — wcore-cli lib.rs/main.rs untouched, ci.yml/release.yml untouched (verified by diff vs 75babf32)
status: >-
  root cause proven and fixed; suite deterministic at default concurrency;
  the reported 22/18-at-default-concurrency observation remains unreproduced
  and is explained but not confirmed
---

# Flaky suite root cause — `cargo nextest run -p wcore-agent --lib`

Base `75babf32`. All builds and runs on `hetzner-dsm` (96 cores, `ulimit -Sn` 1024,
`ulimit -Hn` 1048576). Every number below comes from unproxied `/root/.cargo/bin/cargo` and
`/usr/bin/grep` against raw captured logs under `/root/flaky-eviden/`.

## 1. The mechanism

**The failures are not test failures. The tests never ran.**

At `--test-threads=384` on the unchanged tree, the worst run produced:

```
TRY 1 XFAIL  : 96      <- exec failed: nextest could not SPAWN the test process
TRY 1 FAIL   : 0       <- real test-assertion failures
error strings: 107 x "- Too many open files (os error 24)"
```

`XFAIL` at `0.000s` is a failed `fork`/`exec`, not a failed assertion. Across **all 20**
pre-fix runs at 384 threads, `realfail1` was **0 every time**. Not one test in this suite
has been observed to actually fail.

Peak fd usage of the runner process, sampled from `/proc/<runner>/fd` at `ulimit -n 1024`:

| `--test-threads` | peak runner fds | % of limit | outcome |
|---|---|---|---|
| 96 (= `num-cpus`, the default) | **299** | 29% | clean |
| 192 | **569** | 56% | clean |
| 384 | hits the 1024 cap | 100% | 13-81 exec-failures |

Model: `peak ≈ 2.9 × test-threads + ~20`. The 1024 ceiling is crossed at ~346 test-threads.

`cargo-nextest` does **not** raise its own soft limit: `/proc/<runner>/limits` read during a
live run shows `Max open files 1024 1024`.

### Why this presents exactly as a parallelism bug in the tests

- Which test is unlucky enough to request a spawn while the fd table is full is pure
  scheduling, so **the failing set differs run to run in both directions** — the signature
  the brief reports as "not a stable subset".
- `--test-threads=1` needs ~3 fds and is therefore always clean, which reads as "our tests
  share state under parallelism".
- The default profile's `retries = 1` absorbs most of them as `flaky`; only tests that lose
  the race on *both* attempts surface as `exec failed`. That is why the visible count
  (11) is far smaller than the true incidence (96).

## 2. The proof — causal, both directions, no code change

Only `ulimit -n` and `--test-threads` were varied. Same worktree, same commit.

**Force it at DEFAULT concurrency** (`ulimit -n 256`, no `--test-threads` flag):

```
run=1 emfile=3490 execfail_try1=1841 realfail_try1=0   523 passed, 1649 exec failed
run=2 emfile=3565 execfail_try1=1860 realfail_try1=0   467 passed, 1705 exec failed
run=3 emfile=3527 execfail_try1=1863 realfail_try1=0   508 passed, 1664 exec failed
```

**Remove it at the concurrency that broke** (`ulimit -n 65536`, the exact settings that
produced 96 exec-failures at 1024):

```
--test-threads=768 : 3/3 runs — emfile=0 execfail=0 realfail=0, 2172 passed
--test-threads=384 : 3/3 runs — emfile=0 execfail=0 realfail=0, 2172 passed
```

Lowering only the fd budget reproduces the failure at default concurrency; raising only the
fd budget eliminates it at 4x and 8x oversubscription. The fd budget, not concurrency and
not any test's logic, is the controlling variable.

## 3. Why `CLASS-ENV-01` is the wrong answer

The program's standing label is *"process-wide environment mutation in parallel tests"*.
For anything measured under **nextest** that cannot be the mechanism:

1. **nextest runs one process per test.** A `set_var` in test A is invisible to test B —
   it dies with A's process. `--test-threads=1` serialises *processes*, not threads.
2. **In an exec-failure the test body never executes.** There is no window in which any
   shared resource — env var, lease, temp path, port, global registry — could be raced.
3. Measured: 96 first-try failures, **0** of them test failures.

CLASS-ENV-01 is real *for bare `cargo test`*, which is thread-parallel in one process; the
BACKLOG's own `lane/25-cloud` and `lane/core-254` rows were both measured that way and are
sound. But its third row — the `wcore-skills` watcher cluster, "~20 EMFILE failures at
0.007s" — is a **resource-exhaustion finding filed under an env-mutation heading**, and it
is the row whose magnitude matches the flake I was sent to explain.

## 4. Second mechanism, separate resource, same errno

`crates/wcore-agent/src/watch.rs:101` calls `notify::recommended_watcher`, allocating a real
**inotify instance**. Instances are capped **per-user** by `fs.inotify.max_user_instances`
(512 here), and every lane runs as `root`, so that counter is genuinely shared across lanes
in a way `RLIMIT_NOFILE` is not. `inotify_init` returns **EMFILE — errno 24, identical to
the spawn path** — which is precisely how the two got conflated.

Exactly three lib tests construct one (`grep -rn "WatchHandle"` returns 0, so the obvious
name is wrong; the type is `FileWatcher`):

- `crates/wcore-agent/src/watch.rs:487` — `self_write_suppressed_even_when_drained_late_bug2a`
- `crates/wcore-agent/src/file_watcher_notifier.rs:64` and `:94`

They are distinguishable in the log: instance exhaustion panics **inside** a running test
(`TRY 1 FAIL`), fd exhaustion kills the **spawn** (`TRY 1 XFAIL`, 0.000s). Three tests is
not 18-22, so this is a contributor, not the explanation. **Left unfixed and reported**,
because it is a MEDIUM (test-infrastructure, `.unwrap()` on a resource the test does not own)
and I did not want to change behaviour I had not reproduced.

## 5. The fix

`scripts/fd-budget.sh` (new) plus two additive lines in `justfile` (`test`, `test-ci`).

It computes demand (`4 × test-threads + 192`, from the measured 2.9 fds/thread plus
headroom), raises the soft `RLIMIT_NOFILE` toward the hard limit to meet it, and **if it
cannot, refuses to run and names the resource** instead of letting the run proceed into a
nondeterministic EMFILE regime.

- **Not serialisation.** No `#[serial]`, no test-group, no `test-threads` cap. Concurrency
  is untouched. `serialised-tests: none`.
- **A no-op on every host we currently use** — hetzner needs 576 of its 1024; CI runners are
  small. It therefore changes no current behaviour, and is *not* what makes the post-fix
  default-concurrency runs green. Stated plainly so the counterfactual is not overread.
- Windows is an explicit pass-through; no `RLIMIT_NOFILE` governs spawn there.
- `just -n test-ci` verified to expand to
  `scripts/fd-budget.sh vx cargo nextest run --workspace --profile ci --no-fail-fast`.

### The guard's self-test can fail, and did

```
self-test setup: constrained budget established, soft/hard = 64/64
self-test 1 PASS: adequate budget -> guard passes, command runs
self-test 2 PASS: inadequate budget -> guard fails loudly, command suppressed
self-test 3 PASS: unguarded path silently proceeds on the same budget the guard rejects
fd-budget self-test: 3/3 PASS
```

Assertion 3 is the one that proves the guard does anything — without it, 1 and 2 also pass
on a guard that is never consulted. The **setup** assertion exists because the first
revision reported a false FAIL on assertion 2: it lowered the *hard* limit before the *soft*
limit, which is an error leaving the hard limit untouched, so the guard correctly raised the
soft limit and ran the command. The instrument was repaired in this lane rather than written
up and left. Verified 3/3 on both macOS and Linux.

## 6. Repetition — the distribution, before and after

Identical harness, identical ambient soft limit (1024), 20 runs per cell.

| Cell | Runs | Clean | Dirty | Exec-failures per dirty run | Real test failures |
|---|---|---|---|---|---|
| **pre-fix, default (96)** | 20 | **20** | 0 | — | **0** |
| **post-fix, default (96)** | 20 | **20** | 0 | — | **0** |
| **pre-fix, `--test-threads=384`** | 20 | 10 | **10** | 30, 60, 31, 81, 23, 42, 13, 14, 18, 28 | **0** |
| **post-fix, `--test-threads=384`** | 20 | **20** | 0 | — | **0** |

In all 20 post-fix runs at 384 the guard logged `raised RLIMIT_NOFILE` (`raised=1`, 20/20) —
the raise path is exercised, not bypassed.

**The pre-fix 384 spread is 13-81 exec-failures. The brief's reported 18 and 22 sit inside
it.** That is corroboration, not proof, and I am labelling it as such.

Additional clean runs at default concurrency during investigation: 5 baseline + 3 at 192 +
1 fd-sampling + 8 across 4 concurrent suite copies = **37 clean default-concurrency runs,
zero failures**, before the 20+20 above.

## 7. What I could not explain

**I could not reproduce 22/18 failures at default concurrency.** 37 + 40 = 77 runs at
`--test-threads=96` on this tree produced **zero** failures of any kind. Things I ruled out
by measurement:

- **Tree drift** — my base `75babf32` is 1373 commits *ahead* of
  `origin/plan/f20-unified-audit-repair`; it is not an older tree.
- **Multi-lane contention** — 4 concurrent copies of the suite at default concurrency, load
  to 31, 8/8 clean, 0 EMFILE.
- **Starvation/timeout kills** — only 6 of 2172 tests exceed 5s and the long poles are fixed
  network timeouts to TEST-NET-1 (`192.0.2.1`, non-routable by RFC 5737), not CPU work. No
  cohort sits near the 60s kill line.

The most likely explanation, which I could not confirm without the original lanes' capture,
is that those runs happened in a context with a **smaller effective fd budget or a larger
effective thread count** than a plain `ssh hetzner-dsm` shell — a container, a login shell
with different limits, or a host with more cores. `2.9 × threads` versus the ambient soft
limit is the whole predicate, and both terms are environmental. **If someone still has those
logs, `grep -c "TRY 1 XFAIL"` and `grep -c "TRY 1 FAIL"` settles it in one command** — the
first non-zero and the second zero confirms this mechanism outright.

Until then the honest status is: **mechanism proven and fixed; the specific reported
observation explained but not confirmed.**

## 8. Separate finding — an inert anti-vacuity gate (HIGH for evidence integrity)

`cargo nextest list` on this host prints:

```
warning: in config file .config/nextest.toml, ignoring unknown configuration key:
profile.default.no-tests
```

The repo's `no-tests = "fail"` key — documented at length in `.config/nextest.toml` as
closing the whole "exits 0 having certified nothing" family, covering three measured
flavours — **is silently ignored by the installed cargo-nextest 0.9.137.** `vx.toml` pins
nextest as `nextest = "cargo nextest"` with no version, so whichever version is installed
decides. Any lane relying on that key to catch a zero-test run is relying on a gate that is
not running. It did not affect my numbers (I read the executed count back directly, 2172,
and cross-checked against `nextest list` = 2172).

**Not fixed here** — pinning a nextest version is a toolchain/CI decision and `vx.toml` is
release-coordination surface. Flagged for the release owner.

## 9. Fences

- `crates/wcore-cli/src/lib.rs`, `crates/wcore-cli/src/main.rs` — **untouched**
  (`git diff --stat 75babf32 --` empty).
- `.github/workflows/ci.yml`, `release.yml` — **untouched** (same check, empty).
- No merge to main, no PR, no tag, no release, no issue closed, no `wcore-contract generate`.
- Full change surface vs `75babf32`: `scripts/fd-budget.sh` (new, 242 lines), `justfile`
  (+16/-2, two recipe bodies and their comments), `.planning/FLAKY-ROOT-CAUSE-NOTES.md`,
  this file.
- `justfile` is shared with CI. The edit is additive prefix only — no recipe restructured,
  no `[unix]`/`[windows]` split introduced, no reordering.
- One deviation to declare: I ran `git reset --hard` **on hetzner**, in my own scratch
  worktree `/root/wayland-flaky` on my own branch `hz/flaky-root-cause`, to move it to the
  fix commit. It moved only my own ref. No `reset`/`checkout`/`stash`/`rebase` was run in
  the Mac worktree.

## 10. Evidence

On `hetzner-dsm` under `/root/flaky-eviden/`: `baseline/`, `oversub-192|384|768/`,
`forced-lowfd-default/`, `removed-highfd-384|768/`, `multilane/`, `fdpeak-*.log`,
`prefix-default/`, `prefix-t384/`, `postfix-default/`, `postfix-t384/`,
`postfix-t384-raise/`, plus the harnesses `runharness.sh`, `causal.sh`, `fdpeak.sh`,
`multilane.sh`, `rep20.sh`, `rep20fix.sh`, `rep20fix2.sh` and their tally files
`baseline.txt`, `oversub.txt`, `causal.txt`, `multilane.txt`, `prefix.txt`, `postfix.txt`,
`postfix2.txt`.
