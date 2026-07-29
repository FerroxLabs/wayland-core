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

## Still to establish

- [ ] Reproduce git's refusal under root-vs-other-uid ownership on hetzner (no docker needed).
- [ ] Distinguish, by measurement, whether Defect 2's `OverTime` means "cancel is broken"
      or "the pre-stop phase did not fit in 3 s on a loaded box".
