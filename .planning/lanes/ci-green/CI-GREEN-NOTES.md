# Lane `ci-green` — running NOTES

Branch `lane/ci-green`, based on `plan/f20-unified-audit-repair` @ `1097cfb3`.
Committed early per LANE-BRIEF §6b-i. Appended after every measurement.

Scope: 3 of the 5 CI failures in run 30434804220 job `CI (linux-containerized)`.
OUT OF SCOPE (do not touch): `wcore-protocol::desktop_contract_corpus` (both tests).

---

## t0 — orientation (before any measurement)

- Worktree toplevel verified: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-green`.
- HEAD `1097cfb300d19b3524d696cce58ad85d5c7a33fe`.
- `.config/nextest.toml` read. Key facts for Defect 1:
  - `[profile.ci]` does NOT set `run-ignored`; nextest default is to skip `#[ignore]`d tests.
  - `[profile.default] no-tests = "fail"` is inherited by `ci` — an invocation matching ZERO
    tests FAILS. This matters: if I gate the linux test behind `#[ignore]`, a targeted
    proof-host invocation `-E 'test(=on_linux_...)' --run-ignored all` still selects it, so
    `no-tests = fail` cannot silently self-pass.
  - `just test-ci` = `cargo nextest run --workspace --profile ci --no-fail-fast`.
    A workspace run executes thousands of tests, so `no-tests` is not triggered by
    one binary contributing zero.
- Defect 1 test source read: `crates/wcore-exec-backend/tests/node_contract.rs:143-185`.
  Confirmed the precondition loop asserts `HOSTNAME` unset with "Failing rather than skipping."
- Defect 2 site read: `crates/wcore-cli/tests/deterministic_openai_loop.rs:339-395`.
  Scenario budget: `max_total_time(5s)`, turn `max_time(3s)`, `.stop_mid_turn()`.
  Assertion is exact-set `[Failure::CostMissing]`.

## t1 — CI log read back from the source (not from the brief)

Fetched the real job log (`gh api repos/.../actions/jobs/90552394666/logs`, 32,722 lines).
NOTE: `gh run view --log` is intercepted by `rtk` and returned `rtk: Run ID required`;
used `/opt/homebrew/bin/gh api` instead (LANE-BRIEF §3b).

Two complete suite executions, identical outcome both times:
`Summary [478.701s] 12987 tests run: 12982 passed (1 slow, 2 leaky), 5 failed, 50 skipped`
`Summary [471.973s] 12987 tests run: 12982 passed (1 slow, 1 leaky), 5 failed, 50 skipped`
Exactly the 5 named failures. **No sixth failure.**

Verbatim failure payloads:

1. `node_contract.rs:147` — `precondition: HOSTNAME is set, so this run measures the env
   branch, not the file-fallback branch.` → diagnosis CONFIRMED.
2. `deterministic_openai_loop.rs:377` — `expected exactly [CostMissing] after a mid-turn
   cancellation, got [OverTime { observed_secs: 3.000722258, budget_secs: 3.0 }].
   wall_time 3.118363537s, cancellation_requested true, final_text ""`
3. `deterministic_openai_loop.rs:603` — `seal exact F04 Core artifact: Probe("expected source
   identity is not 40 lowercase hexadecimal characters")`. Origin string is `"expected"`
   (artifact.rs:197 `validate_commit(expected.source_commit, "expected")`), i.e. the
   EXPECTATION, which comes from `env!("WAYLAND_SOURCE_SHA")` — so the build script embedded
   `"unknown"`. → mechanism CONFIRMED at the right layer.

## t1 — CORRECTION to the brief's reading of Defect 2

`observed_secs: 3.000722258` is NOT "overshot the budget by 0.7 ms". It is
`started.elapsed()` sampled **after** `tokio::time::timeout` fired
(runner.rs:1149-1153), so 0.7 ms is timer wakeup granularity. The real fact is:
**the turn consumed its entire 3.000 s budget.** `wall_time 3.118 s` means spawn+boot+
prompt took ~0.117 s and the turn ate the rest.

Also: `assert!(result.execution.cancellation_requested)` in that test is a TAUTOLOGY.
On the normal path it is `scenario.turns.iter().any(|t| t.stop_mid_turn)` (runner.rs:1043)
— it echoes the test's own config; on BOTH failure paths it is hardcoded `true`
(runner.rs:780, 839). It cannot fail. Self-passing assertion inside the test under repair.

## t1 — Defect 3: the release path is NOT broken (measured, refutes my first guess)

I assumed `cross build` in `release.yml` would hit the same container/git problem and ship
`source unknown`. **Measured and REFUTED.** Downloaded v0.12.25 assets, ran the x86_64 one
on hetzner:

```
$ /root/ci-green-relbin-x86_64 --build-info
wayland-core 0.12.25 (source 61b79c4)      rc=0
```

Both the native (x86_64) and the `cross`-built (aarch64) binaries carry `61b79c4`, the real
short sha of tag v0.12.25 (`61b79c4f90f71fe2cf243affa7620b3c9b607f14`). Seven chars because
build.rs at that tag used `git rev-parse --short HEAD`; `94064e66 fix(cli): embed full source
identity` later moved it to full 40-hex. So `cross` does not break git — it runs the
container as the invoking uid.

That isolates the CI failure to the `ci-linux` job specifically: its `docker run` has **no
`-u`**, so it runs as **root** against a workspace owned by the runner uid (1001).
`actions/checkout` adds a `safe.directory` entry for `/home/runner/work/...` in the RUNNER's
git config (log lines 96-97) — a different path, a different HOME, a different uid.

## t2 — Defect 3 mechanism reproduced on hetzner (with a known-positive control)

```
--- as root, repo owned by root (control, expect SUCCESS):
90ef9c71af7b39087936c1a1c313b8d5b5c77956      rc=0
--- as root, same repo chowned to uid 1001 (expect FAILURE):
fatal: detected dubious ownership in repository at '/tmp/cigreen-own'      rc=128
```

That is the `ci-linux` shape exactly: `docker run` with no `-u` runs as root over a
workspace owned by the runner uid. `git_output()` filters on `status.success()`, so
`resolve_source_sha` takes the `Ok("unknown")` arm.

Also established: `wcore-cli` / `wayland-core` do **not** exist on crates.io (queried with
a UA; `crate ... does not exist`), and **no workflow runs `cargo publish`**. So there is no
source-distribution build path that a fail-closed release rule could break.

## t3 — Defect 1 fixed and proven on hetzner (`bf9fe2b8`)

Rewrote both `local_identity` tests to derive `NodeIdentity::local` in a CHILD PROCESS with
`WAYLAND_NODE_MACHINE_ID` / `HOSTNAME` / `COMPUTERNAME` removed, instead of asserting them
absent in the ambient environment.

| arm | invocation | result |
|---|---|---|
| A — proof-host shape | `env -u HOSTNAME … cargo nextest run … node_contract` | `19 tests run: 19 passed, 1 skipped` |
| B — the CI shape that was red | `HOSTNAME=deadbeefc0de cargo nextest run …` | `19 tests run: 19 passed, 1 skipped` |
| Darwin (Mac, narrow LANE-BRIEF exception) | `cargo test -p wcore-exec-backend --test node_contract` | `19 passed; 0 failed; 1 ignored` |

Arm B is its own negative control: the derived value was
`machine_id=ubuntu-2404-noble-amd64-base` (from `/etc/hostname`), **not** `deadbeefc0de`.
If the env cleaning leaked, the new exact-match assertion would have failed.

KNOWN-NEGATIVE (mutated `read_hostname_file` to read two nonexistent paths, simulating the
Darwin defect spreading to Linux):
```
assertion `left != right` failed: on Linux the fallback must find the hostname file.
  Reading 'unknown-host' here would mean the Darwin defect has spread.
Summary [0.897s] 13/19 tests run: 12 passed, 1 failed, 1 skipped     KN_EXIT=100
```
Reverted; `19 tests run: 19 passed` again. `REVERT_EXIT=0`.

## t4 — Defect 2 RESOLVED BY MEASUREMENT, and the brief's hypothesis (a) is right for a
##      different reason than stated

Reproduced first: 48 busy loops on the 96-core box, 30 reps →
**`TOTAL pass=15 fail=15`.** Not a 2.5% flake under contention — a 50% one.

Then traced it (`WCORE_EVAL_TURN_TRACE=1`, the instrument added in `de47947b`):

```
PASSING (idle):
  t=0.000s prompt_sent
  t=0.000s  … 26 bootstrap events …
  t=1.889s event=stream_start        <-- 1.9s of engine work before the provider request
  t=1.928s event=provider_attempt
  t=1.930s event=text_delta
  t=1.930s stop_sent
  t=1.931s event=stream_end          <-- cancellation honoured in ~1 MILLISECOND
  t=1.931s turn_end stop_pending=false

FAILING (under load), every single one:
  t=3.001s TURN_TIMEOUT stop_pending=TRUE
```

`stop_pending=true` on **every** failure means the stop command was NEVER SENT: the 3s turn
budget expired before a first token existed to cancel on. Across 20 traced runs there is
**not one** observation of a stop that was sent and not honoured.

So `Failure::OverTime` in this test never measured cancellation. It measured
time-to-first-token, which is ~1.9-2.1s on an idle 96-core box against a 3.0s budget — under
a second of slack. The neighbouring packaged scenarios in the same file already use 10s/20s.

## t5 — Defect 2 fix proven (`e87c7baf`)

Fixture stall 10s → 60s; turn budget 3s → 15s; scenario 5s → 20s; and a NEW load-bearing
assertion bounding `turn.wall_time - first_token_time` (the cancellation latency) at 1s.

| measurement | base `1097cfb3` | fixed `e87c7baf` |
|---|---|---|
| 30 reps, 48-core load | `TOTAL pass=15 fail=15` | — |
| 20 reps, 48-core load | — | `LOAD_TOTAL pass=20 fail=0 flaky=0` |

Worst `stream_start` observed under load: **t=3.329s** — i.e. above the old 3.0s budget,
which is the failure directly. Worst `turn_end`: t=3.378s, so cancellation latency stayed
≤49ms against the 1s ceiling and the 60s stall.

KNOWN-NEGATIVES (mutating `main.rs:5099` so the engine delays honouring `Stop`; reverted
after each, working tree confirmed clean):

- **3s delay** → `cancellation did not abort the active stream promptly: stop was sent on
  the first text delta at 1.894501386s and the turn did not end until 4.896622323s
  (3.002120937s later) … failures: [CostMissing]` → `0 passed, 1 failed`.
  Note `failures: [CostMissing]` — **every pre-existing assertion still passed.** Only the
  new latency assertion caught it. That is the third assertion LANE-BRIEF §6b-ii requires:
  the old instrument would have missed a three-second cancellation stall.
- **30s delay** → `Summary [45.837s] 1 test run: 0 passed, 1 failed`. The 15s turn budget
  still catches a wholesale cancellation failure, so raising it from 3s did not remove the
  safety net.
