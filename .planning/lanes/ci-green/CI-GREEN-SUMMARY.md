# Lane `ci-green` — SUMMARY

Branch `lane/ci-green`, based on `plan/f20-unified-audit-repair` @ `1097cfb3`.
Scope: three of the five failures in CI run 30434804220, job `CI (linux-containerized)`.
**Out of scope and deliberately left red:** both `wcore-protocol::desktop_contract_corpus`
tests. Not touched — see "What I did NOT do".

---

## Verdict per defect

| # | Defect | Orchestrator's diagnosis | Verdict |
|---|---|---|---|
| 1 | `node_contract` `HOSTNAME` precondition | correct | **FIXED + proven, 3 arms + known-negative** |
| 2 | `packaged_core_cancels_an_active_stream` | *directionally* right, mechanism wrong | **FIXED + proven, 20/20 under load + 2 known-negatives** |
| 3 | `packaged_f04…` source identity | mechanism correct, blast radius wrong | **FIXED (both halves) + mechanism reproduced** |

---

## Defect 1 — `on_linux_local_identity_reads_a_real_per_host_value_from_the_hostname_file`

**Diagnosis held.** CI log verbatim:
`precondition: HOSTNAME is set, so this run measures the env branch, not the file-fallback
branch.` GitHub Actions containers export `HOSTNAME`, so the test could never pass there.

**What I did NOT do: `#[ignore]`.** The brief suggested it and I checked the ground first.
`.config/nextest.toml`'s `[profile.ci]` does not set `run-ignored`, so `#[ignore]` would
indeed have hidden it from CI — but it would also have made the test unrunnable anywhere
except a hand-crafted proof-host invocation, and left the Darwin twin carrying the identical
ambient-environment defect.

**What I did instead:** the two `local_identity` tests now derive `NodeIdentity::local` in a
**child process** with `WAYLAND_NODE_MACHINE_ID` / `HOSTNAME` / `COMPUTERNAME` removed, and
read the values back as `PROBE_*=` lines. Same re-exec idiom as
`crates/wcore-swarm/tests/workspace_authority.rs`. In-process `env::remove_var` was not an
option: it is `unsafe` and process-global, and this binary runs tests in parallel threads.

This keeps *both* properties the author wanted and adds a third:
- nothing is skipped and nothing is conditional — the child always runs, the parent always
  asserts, and a violated precondition still fails loudly;
- it runs on the proof host over non-login ssh;
- **it now also runs, and passes, inside a CI container** rather than being excluded.

Anti-vacuity guard: `libtest` exits 0 printing `0 passed` when `--exact` matches no test, so
the parent asserts all six `PROBE_*` keys are present. A renamed fixture fails rather than
silently asserting nothing.

I also strengthened the Linux assertion from "not the constant `unknown-host`" to an exact
match against the sanitized contents of this host's hostname file — otherwise any non-empty
string satisfied it.

### Proof

| arm | invocation | result |
|---|---|---|
| A — proof-host shape, non-login ssh | `env -u HOSTNAME -u COMPUTERNAME -u WAYLAND_NODE_MACHINE_ID cargo nextest run -p wcore-exec-backend --test node_contract --profile ci` | `19 tests run: 19 passed, 1 skipped` |
| B — the CI shape that was red | `HOSTNAME=deadbeefc0de cargo nextest run …` | `19 tests run: 19 passed, 1 skipped` |
| Darwin — Mac, narrow LANE-BRIEF exception | `cargo test -p wcore-exec-backend --test node_contract` | `19 passed; 0 failed; 1 ignored` |

Arm B is its own negative control: the derived value was
`machine_id=ubuntu-2404-noble-amd64-base` (from `/etc/hostname`), **not** `deadbeefc0de`.
A leak in the env cleaning fails the exact-match assertion.

**KNOWN-NEGATIVE** — mutated `read_hostname_file` to read two nonexistent paths (simulating
the Darwin defect spreading to Linux):
```
assertion `left != right` failed: on Linux the fallback must find the hostname file.
  Reading 'unknown-host' here would mean the Darwin defect has spread.
Summary [0.897s] 13/19 tests run: 12 passed, 1 failed, 1 skipped        KN_EXIT=100
```
Reverted → `19 tests run: 19 passed`, `REVERT_EXIT=0`.

**Mac usage disclosed:** the Darwin arm was run on the Mac with
`cargo test -p wcore-exec-backend --test node_contract` — single crate, single test file,
Darwin-only behaviour (`/etc/hostname` and `/proc` absent) that no permitted host can show.

---

## Defect 2 — `packaged_core_cancels_an_active_stream`

**The brief's reading of the number was wrong, and it changed the answer.**
`observed_secs: 3.000722258` is not "overshot the budget by 0.7 ms". It is
`started.elapsed()` sampled *after* `tokio::time::timeout` fired (`runner.rs:1149`), so
0.7 ms is timer wakeup granularity. The turn consumed its **entire** 3.000 s budget.

I built the instrument before choosing (LANE-BRIEF §6b-ii): a `WCORE_EVAL_TURN_TRACE`
turn trace in `wcore-eval-scenarios::runner`, which is permanent.

```
PASSING (idle):                          FAILING (48-core load), EVERY one:
  t=0.000s prompt_sent                     t=3.001s TURN_TIMEOUT stop_pending=TRUE
  t=0.000s … 26 bootstrap events …
  t=1.889s event=stream_start
  t=1.928s event=provider_attempt
  t=1.930s event=text_delta
  t=1.930s stop_sent
  t=1.931s event=stream_end   <-- ~1 MILLISECOND
  t=1.931s turn_end stop_pending=false
```

`stop_pending=true` on every failure means the `stop` was **never sent** — the budget
expired before a first token existed to cancel on. Across 20 traced runs there is not one
observation of a stop that was sent and not honoured. **`Failure::OverTime` in this test has
never measured cancellation.** It measured time-to-first-token: ~1.9–2.1 s idle, worst
observed 3.329 s under load, against a 3.0 s budget.

### Decision: (a), the assertion — but not the assertion the brief proposed

The brief's (a) was "assert `CostMissing` is present and no other *class* of failure
occurred, rather than exact set equality". **I rejected that.** Under the fixture's stall,
`OverTime` is also what a genuinely-broken cancellation would produce, so admitting it would
have deleted the test's only failure channel. Exact set equality is retained.

What I changed instead is the *interval being budgeted*:
- fixture stall `10s → 60s` (the value `f14_sigkill_recovery` already uses);
- turn budget `3s → 15s`, scenario `5s → 20s` — in line with this file's own other packaged
  scenarios (10s/20s); still far below the stall, so an uncancelled stream is still caught;
- **new load-bearing assertion**: `turn.wall_time - first_token_time < 1s`. That is the
  cancellation latency, measured independently of how long the engine took to reach the
  stream. Both fields already existed; no new plumbing.

This is a *raised* timeout, which LANE-BRIEF §5 flags. I claim it is not a weakening and
proved it: the discriminating margin went from 10s-stall-vs-3s-budget (3.3x) to
60s-vs-15s (4x), the false-positive margin went from 1.03x to 4.5x, and a tight 1 s direct
bound on the real property was added where there was none.

### Proof

| measurement | base `1097cfb3` | fixed |
|---|---|---|
| 30 reps, 48 busy cores | `TOTAL pass=15 fail=15` | — |
| 20 reps, 48 busy cores | — | `LOAD_TOTAL pass=20 fail=0 flaky=0` |

**KNOWN-NEGATIVES** — `sleep(N)` inserted at `main.rs:5099` before the engine honours
`Stop`; reverted after each, working tree confirmed clean:

- **3 s** → `cancellation did not abort the active stream promptly: stop was sent on the
  first text delta at 1.894501386s and the turn did not end until 4.896622323s
  (3.002120937s later) … failures: [CostMissing]` → `0 passed, 1 failed`.
  Note `failures: [CostMissing]`: **every pre-existing assertion still passed.** Only the new
  latency assertion caught it. That is the third assertion §6b-ii demands — the old
  instrument would have missed a three-second cancellation stall outright.
- **30 s** → `Summary [45.837s] 1 test run: 0 passed, 1 failed`. The 15 s turn budget still
  catches a wholesale cancellation failure, so raising it did not remove the safety net.

### Separate finding (not fixed, flagged)

`assert!(result.execution.cancellation_requested)` in this test **cannot fail**. On the
normal path it is `scenario.turns.iter().any(|t| t.stop_mid_turn)` (`runner.rs:1043`) — it
echoes the test's own configuration — and it is hardcoded `true` on both failure paths
(`runner.rs:780`, `:839`). A self-passing assertion inside the test under repair. Left in
place with an explicit comment saying so rather than deleted.

---

## Defect 3 — `packaged_f04_run_is_repeatable_and_content_addressed`

**Mechanism confirmed at the right layer.** The failure string is
`Probe("expected source identity is not 40 lowercase hexadecimal characters")` and the
origin word is `"expected"` — `artifact.rs:197` validates `expected.source_commit`, which
comes from `env!("WAYLAND_SOURCE_SHA")`. So the build script embedded `"unknown"`.

**Reproduced the cause directly on hetzner, with a known-positive control in the same run:**
```
--- as root, repo owned by root (control):        90ef9c71…   rc=0
--- as root, same repo chowned to uid 1001:
fatal: detected dubious ownership in repository at '/tmp/cigreen-own'   rc=128
```
`ci-linux`'s `docker run` carries no `-u`, so it runs as root over a workspace owned by the
runner uid 1001. `actions/checkout` *does* add a `safe.directory` exception — for
`/home/runner/work/...` in the **runner's** git config, a different path, uid and HOME than
the container's. git is installed in the image, so "git unavailable" was not it.

**Where the brief's blast radius was wrong — and it matters.** I expected the shipped
release binaries to carry `source unknown` too, since `release.yml` builds aarch64 through
`cross` and never sets `WAYLAND_BUILD_SOURCE_SHA`. **Measured and refuted.** I downloaded the
v0.12.25 assets and ran the x86_64 one on hetzner:
```
$ /root/ci-green-relbin-x86_64 --build-info
wayland-core 0.12.25 (source 61b79c4)     rc=0
```
Both the native and the `cross`-built binary carry `61b79c4`, the real short sha of tag
v0.12.25 (`61b79c4f90f71fe2cf243affa7620b3c9b607f14` — seven chars because build.rs at that
tag still used `--short`). `cross` runs its container as the invoking uid, so git works.
**The release path is not and was not leaking `unknown`.**

### Both halves addressed

**(1) The CI job.** `-e WAYLAND_BUILD_SOURCE_SHA=${{ github.sha }}` added to *both*
`DOCKER_RUN` and `DOCKER_RUN_SANDBOX`, so every containerized step agrees (a differing value
between the pre-build and the test step would re-trigger `rerun-if-env-changed` and rebuild
wcore-cli for nothing). Rejected alternative, recorded in the file: `-e GIT_CONFIG_*` adding
`safe.directory=/work`, which repairs git-in-container generally but leaves provenance
*derived* rather than pinned. This also fixes a latent second failure: the containerized
`F01 packaged wayland-eval driver gate` step compares `github.sha` against a binary that was
built with `unknown`; it was never reached because the test step failed first.

**(2) The silent `Ok("unknown")` fallback — fixed now, not written up.** Release builds
(`PROFILE=release`) with neither `WAYLAND_BUILD_SOURCE_SHA` nor a working git now **fail
closed** with an actionable message. Debug builds keep the fallback.

I chose "fix now" because the change is contained and I could show the blast radius is
empty: neither `wcore-cli` nor `wayland-core` exists on crates.io (queried directly:
`crate ... does not exist`), and **no workflow anywhere runs `cargo publish`**. Every
release artifact is built from a git checkout or from CI, and CI can always pass the
variable. There is therefore no source-distribution path a hard failure could break. Every
`--release` invocation across all eleven workflows was checked; the only one that lacked a
working git is the one this lane just pinned.

`resolve_source_sha` takes `PROFILE` as a parameter rather than reading the env inside, so
the policy is unit-tested. `build_script_provenance` went 5 → 8 tests, all passing, including
a release-fails-closed case, a release-still-succeeds counterpart (so the refusal is about
the missing identity, not the profile), and an unknown-`PROFILE` case that must not be read
as release.

---

## Gate results

*(filled in below from the full-suite run)*

## Files changed

| file | why |
|---|---|
| `.github/workflows/ci.yml` | pin `WAYLAND_BUILD_SOURCE_SHA` into both containerized docker invocations |
| `crates/wcore-cli/build.rs` | release builds fail closed on an unattributable source identity |
| `crates/wcore-cli/tests/build_script_provenance.rs` | 3 new cases for the fail-closed policy |
| `crates/wcore-cli/tests/deterministic_openai_loop.rs` | cancellation-latency assertion; budgets rescoped |
| `crates/wcore-eval-scenarios/src/runner.rs` | `WCORE_EVAL_TURN_TRACE` turn trace |
| `crates/wcore-exec-backend/tests/node_contract.rs` | derive identity in a cleaned child environment |

**Shared-file fence:** `crates/wcore-cli/src/lib.rs` and `src/main.rs` are **untouched** in
every commit on this branch. `main.rs` was mutated only transiently on hetzner for the two
known-negatives and reverted; `git status --porcelain` came back empty afterwards.
