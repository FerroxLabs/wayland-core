# lane/fix-clippy-gate — NOTES

Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (integration head at spawn).
hetzner worktree: `/root/wayland-fix-clippy-gate`, branch `hz/fix-clippy-gate`, SHA asserted
equal to the above.

## 1. Premise check — the brief was WRONG, and wrong in a specific, instructive way

The brief said: `cargo clippy --workspace --all-targets -- -D warnings` fails with
**"exactly one failing file, two errors"** — `crates/wcore-cron/tests/single_owner.rs`.
It also said two earlier lanes had reported
`crates/wcore-agent/tests/cache_ledger_engine_test.rs:82` (`needless_update`) and that it
"did NOT appear in my run", speculating it had been fixed by a merge.

Measured at the identical SHA. **The brief's run was a fail-fast run.**

- `cargo clippy --workspace --all-targets -- -D warnings` → `WLRC=101`, log 688 lines,
  ends with `warning: build failed, waiting for other jobs to finish...`.
  cargo **stops scheduling new units** at the first failing crate, so the crates after
  `wcore-cron` in the schedule were never linted at all.
- Re-run at the same SHA with `--keep-going` → `WLRC=101`, log 219 lines,
  **10 lint errors across 5 test targets in 4 crates.**

So the earlier lanes were right and the brief was wrong: `cache_ledger_engine_test.rs:82`
**is still present at `e7bc6d88`**. It was invisible to the brief's run because cargo
aborted before `wcore-agent`'s test targets were scheduled — not because a merge fixed it.

**Generalisable instrument defect, worth recording:** *a fail-fast clippy run is a LOWER
BOUND, not an inventory.* Anyone enumerating clippy debt must pass `--keep-going`, or they
will under-report and then wrongly conclude that another lane's finding was stale. This is
the same family as the `--no-fail-fast` nextest drift already documented in `ci.yml`
(one failure reported where there were three).

## 2. The complete, true list at `e7bc6d88` (from the `--keep-going` capture)

| # | Crate | Target | Lint | Site |
|---|-------|--------|------|------|
| 1 | wcore-cron | test `single_owner` | `clippy::zombie_processes` | `tests/single_owner.rs:505` |
| 2 | wcore-cron | test `single_owner` | `clippy::collapsible_if` | `tests/single_owner.rs:514` |
| 3 | wcore-memory | lib test | `non_snake_case` | `src/activation.rs:198` |
| 4 | wcore-browser | test `process_count_reaper_baseline_test` | `clippy::collapsible_if` | `tests/process_count_reaper_baseline_test.rs:99` |
| 5 | wcore-agent | test `cache_ledger_engine_test` | `clippy::needless_update` | `tests/cache_ledger_engine_test.rs:82` |
| 6-9 | wcore-agent | test `user_model_identity_wire` | `clippy::needless_borrows_for_generic_args` ×4 | `tests/user_model_identity_wire.rs:229,337,396,472` |

Nine lint errors, five `could not compile` summary lines, `WLRC=101`.

Raw captures: `evidence/base-clippy.log` (fail-fast), `evidence/kg-clippy.log` (`--keep-going`).

## 3. Is #1 a real defect or a lint nit? — REAL

`spawn_child` (`single_owner.rs:504`) spawns the test binary as a child, polls up to
300×50 ms for a `STARTED` marker, and:

- on success `return child` — every caller (`a`, `b`, `c`) later `.wait()`s it. Fine.
- on timeout **`panic!`s with `child` still live and never reaped**.

The leaked child is not inert: it holds the `ScheduleLease` on the schedule directory and
spins for up to 30 s waiting for a `.release` file that the panicking parent will never
write. Meanwhile the parent's `tempfile::tempdir()` is dropped and the tree removed, so the
child's own `std::fs::write(...).unwrap()` at `single_owner.rs:500` then panics into a
deleted directory. Net effect on a timeout: an orphaned process holding a lease on a path
that no longer exists.

That is exactly what `zombie_processes` exists to catch. **Do not `#[allow]` it.** Fix:
kill + reap before panicking.

## 4. Gate-gap finding — the brief's hypothesis is ALSO wrong

The brief suspected "CI runs a NARROWER scope than `--workspace --all-targets`".
It does not. `.github/workflows/ci.yml:602` runs the exact command:

    cargo clippy --workspace --all-targets -- -D warnings

and `ci.yml` fires on `push` to `plan/f20-unified-audit-repair` and `lane/**`
(`ci.yml:26,41`). So CI scope is CORRECT and CI *is* configured to run on integration.

### 4a. The actual root cause — integration CI is cancelled before any job exists

`ci.yml` almost never *reaches* the clippy step on the integration branch. Its concurrency
group is `${{ github.workflow }}-${{ github.ref }}` with `cancel-in-progress: false` for
branch pushes — deliberate, so the parallel `lane/**` fan-out does not cancel itself. But
`cancel-in-progress: false` does not mean "run them all": GitHub holds at most **one**
pending run per group and **cancels the older pending one** when a newer run enters.
Integration receives merges far faster than a full `ci.yml` run completes, so each queued
run is superseded before a single job is created.

Measured via the REST API (`repos/FerroxLabs/wayland-core/actions/runs?branch=plan%2Ff20-unified-audit-repair`):

| window | runs | cancelled | failure | success | pending |
|--------|------|-----------|---------|---------|---------|
| 2026-07-30 | 45 | 42 | 1 | 0 | 2 |
| last 100 runs (since 2026-07-29T06:22Z) | 100 | 91 | 5 | 2 | 2 |

**One verdict out of 43 completed runs on the day the debt landed.** Four cancelled runs
sampled individually (`30546645108`, `30546311676`, `30541522448`, `30536602951`) all report
`jobs.total_count == 0` — cancelled at the concurrency gate before any job existed. Each
run's `updated_at` matches the next run's `created_at` to within a second, which is the
supersede mechanism visible in the timestamps.

`ci.yml`'s own comment already recorded the symptom ("The integration branch got NINE macOS
jobs in 10.8 h and produced ZERO CI verdicts in 9 h") without naming the cause.

**Fix:** `.github/workflows/lint.yml` — the cheap half of `ci.yml` (fmt + clippy, Linux only,
no matrix, no tests) with `cancel-in-progress: TRUE`, which inverts the failure mode: a burst
of merges cancels the *stale* lint runs and always leaves the *current* head with a verdict in
flight. `ci.yml` is untouched.

## 5. The gate's own first run found a SECOND defect: the vx pin does not hold in CI

Run **30548558624** (`lane/fix-clippy-gate` @ `1eee4ce8`) went **RED on a tree that clippy
passes cleanly on the build host**. That is the §3b-iii shape — a gate with no reachable pass
state — and I nearly shipped it.

Its own log says why, in the `loonghao/vx@v0.9.17` setup step:

```
info: latest update on 2026-07-16 for version 1.97.1 (8bab26f4f 2026-07-14)
info: default toolchain set to stable-x86_64-unknown-linux-gnu
  stable-x86_64-unknown-linux-gnu installed - rustc 1.97.1 (8bab26f4f 2026-07-14)
```

and the lint it then reported cites `rust-clippy/rust-**1.97.0**` at
`crates/wcore-agent/src/workflow_synth.rs:388` (`clippy::question_mark`) — against a
`vx.toml` that pins `rust = "1.95.0"`.

**AGENTS.md claims:** *"The justfile and CI workflows route every tool invocation through vx
so the Rust + just versions pinned in vx.toml are used deterministically across local dev and
CI."* **That claim is false in CI.**

It is worse than a vx.toml miss. This repo also carries `rust-toolchain.toml` with
`channel = "1.95.0"` and `components = ["clippy","rustfmt"]`, and rustup honours it — verified
on hetzner, where the *default* toolchain is `stable` = **1.96.0** yet `rustc --version` inside
the worktree returns **1.95.0** and `cargo clippy --version` returns `clippy 0.1.95`. vx
defeats even that: the action prepends `$HOME/.vx/bin` to `PATH` and the `cargo` found there is
a real toolchain binary rather than the rustup shim, so `rust-toolchain.toml` is never
consulted. No `RUSTUP_TOOLCHAIN` is exported — I grepped the log; the override is purely by
PATH.

**Blast radius beyond this lane.** `ci.yml`'s clippy step is unaffected *only* because it runs
inside `rust:1.95-slim-bookworm` and never touches vx. Every `ci.yml` job that DOES use the vx
setup action — the native `ci` matrix (macOS/Windows/Linux native), `build`, the eval-gate — is
compiling and testing on floating `stable`, currently 1.97.1. For a program that gates releases
on that matrix, the toolchain drifts with upstream and nothing says so. **Reported, not fixed
here** — it is a shared-CI change of a different shape and belongs to whoever owns `ci.yml`.

**Fallout already visible:** `crates/wcore-agent/src/workflow_synth.rs:388` fails
`clippy::question_mark` on 1.97 but not on 1.95. Whoever bumps the pin will need it. That is a
**lower bound** — the 1.97 run was fail-fast, and this lane's own §1 is about exactly that
error.

**How `lint.yml` handles it:** drops vx entirely, lets `rust-toolchain.toml` resolve, then
(a) cross-checks the two pin files and refuses to lint if they disagree, and (b) asserts the
toolchain that actually resolved is the pin. Both controls can fail and both currently pass.

## 6. Both-directions proof of the gate

Two refs differing by exactly one injected `clippy::collapsible_if` (20 lines in
`crates/wcore-cron/tests/single_owner.rs`), so the runs differ by only the thing under test.
The negative control lives on a throwaway ref that is never merged.

| direction | ref | SHA | run | rustc resolved | conclusion | failing step |
|-----------|-----|-----|-----|----------------|-----------|--------------|
| CAN PASS | `lane/fix-clippy-gate` | `c86b6d07` | 30550499711 | 1.95.0 | **success** | — |
| CAN FAIL | `lane/fix-clippy-gate-negctl2` | `c4371a99` | 30550569317 | 1.95.0 | **failure** | Clippy, exit 101 |

The red run's message is the injected lint and nothing else:
`error: this if statement can be collapsed --> crates/wcore-cron/tests/single_owner.rs:516:5`,
`rust-clippy/rust-1.95.0/index.html#collapsible_if`.

Re-proven on the final `rust-toolchain.toml` cut — see SUMMARY.

## 7. `wcore-agent --lib` is red at integration head — pre-existing, and NOT mine

`cargo test -p wcore-agent --lib` fails at base and at my SHA, with a **non-deterministic**
failure count, all in the session-journal writer-lease family
(`AlreadyOwned { lease_path: ... }` / `session journal writer lease is already held`).

| SHA | run | passed | failed | total |
|-----|-----|--------|--------|-------|
| `e7bc6d88` (BASE, isolated worktree) | default threads | 2230 | **22** | 2252 |
| `1eee4ce8` (mine, in the gate sweep) | default threads | 2239 | **13** | 2252 |
| `1eee4ce8` (mine, isolated re-run) | default threads | 2235 | **17** | 2252 |
| `1eee4ce8` (mine) | `-- --test-threads=1` | **2252** | **0** | 2252 |

Two independent proofs it is not mine:

1. **Structural** — `git diff e7bc6d88 HEAD --name-only` touches no file under
   `crates/wcore-agent/src/`; only two integration-test files in that crate.
2. **Empirical** — base fails *worse* (22) than my SHA (13/17), and the constant total
   (2252) shows it is the same suite throughout.

And a clean diagnosis: **serially the suite is 2252/0.** The failures are intra-binary
parallel/timing sensitivity in the journal-lease layer, not cross-lane `/tmp` contention and
not a regression. CI never sees it because `just test-ci` runs `cargo nextest`, which is
process-per-test.

**This is a real finding for another lane:** the crate's own lib suite cannot be run with
`cargo test` at integration head, and the 13/17/22 spread means anyone using it as a gate is
reading noise.

## 8. LANE RESULT

Both tasks achieved. Three defects found that nobody asked for: the reason integration CI
never reports (§4a), a CI toolchain pin that does not hold (§5), and a green check that means
"zero tests ran" (§9).

### Adopted vs authored

- **Adopted verbatim:** `crates/wcore-cron/tests/single_owner.rs` from
  `lane/fix-windows-residuals` @ `f923161b`. We had independently written the *same code* —
  `mut child`, the let-chain, `kill()` + `wait()` before the panic. Diffing mine against
  theirs left **only the comment**, so I took theirs whole:
  `git diff f923161b -- crates/wcore-cron/tests/single_owner.rs` is **empty**, against a
  known-positive of 20 changed lines versus base. Their comment is better — it names the
  cascade and the Windows no-process-group point. Patch: `evidence/adopted-f923161b.patch`.
- **Authored:** items 3, 4, 5, 6-9 of the §2 table. No `#[allow]`, nothing suppressed.
- **Item 3 (`wcore-memory/src/activation.rs`, `non_snake_case`) was found by no other lane** —
  it is in a *lib* test target, and `26-SC2-PEERS-SUMMARY.md` named it "pre-existing, out of
  scope, not fixed" on the 26th.

### Exhaustiveness proof

At the final SHA, on a `cargo clean`-ed tree,
`cargo clippy --workspace --all-targets --keep-going -- -D warnings` = **0**
(`evidence/final-gate-status.txt`). Clean-slate, so a real re-lint of every unit, not a
cached replay.

### Gate 1 — `.github/workflows/lint.yml`, both directions on the shipped version

Two refs differing by exactly one injected `collapsible_if` (20 lines).

| direction | ref | SHA | run | rustc | conclusion |
|---|---|---|---|---|---|
| CAN PASS | `lane/fix-clippy-gate` | `e05ee33a` | 30551783333 | 1.95.0 | **success** |
| CAN FAIL | `lane/fix-clippy-gate-negctl2` | `2a11e7a4` | 30551805485 | 1.95.0 | **failure** at Clippy, 101 |

Red run's message is the injected lint and nothing else. Four further green Lint runs on
later commits — the pass state is not a one-off. Ledger: `evidence/lint-runs.txt`.

### Gate 2 — `report` must not go green on zero tests

Measured on **14 completed CI runs** (3 integration, 11 across 11 lane branches): every one
`run = failure, report = SUCCESS, publish = skipped`, zero counter-examples. Cleanest sample
is my own clippy negative control, run `30550569196`. No known-positive exists — in the last
93 completed CI runs `Publish test report` never executed once, so I claim only that the
*absence* of evidence has been rendering green. Evidence:
`evidence/report-green-on-zero-tests.txt`. Fix is a hard-failing step in `report`,
self-tested five ways.

### Gate 3 — `.planning/scripts/merge-test-gate.sh` + `.planning/merge-test-baseline.txt`

Cost measured, not estimated: full-workspace nextest = **216 s loaded** (137 s compile + 77 s
execution, 13,562 tests / 592 binaries) and **76 s warm**. Reverse-dep selection measured:
closure of `{wcore-config, wcore-agent}` is **34/57 crates**, `wcore-cli` **is** in it so it
*would* have caught the keyring incident — but 60% of the workspace to save ~30 s, with the
saving inversely correlated with risk, is a bad trade. Smoke subset discarded: you cannot
pick it in advance. Differential rather than all-green because integration is red on 3 tests
today and an all-green gate would have no reachable pass state.

Both directions, replaying the **real** incident rather than a synthetic seed:

| arm | baseline | result | time |
|---|---|---|---|
| CAN PASS | committed (3 entries) | **rc 0**, 13,562 executed, 3 = 3, `GATE PASSED` | 89 s |
| CAN FAIL | as it stood before `c73ac417` | **rc 1**, names exactly `wcore-cli::f14_sigkill_recovery isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` | 77 s |

Transition pinned to the merge: at `b8311575` (head immediately before the keyring merge)
that test **passes**, 11/11. So the gate catches it *at* `c73ac417`.
Self-test **12/12** on both GNU sed and BSD sed.

### Defects in my own instruments, repaired in this lane (§6b-ii)

1. Failure-ID matcher under-reported on a space-padded progress counter — one baseline entry
   that could never match, invisibly. Arm E2 is that exact line.
2. The repair was GNU-only: BRE `\?` matched on hetzner and matched **nothing** under BSD
   sed, i.e. a clean pass on any BSD host. Now portable ERE, self-test run on both.
3. Reporter word-split a spaced test ID across two lines. Arm R1 pins it.

Each carries the third assertion — the old matcher would have missed it.

### What I did NOT do

- Did not fix the vx toolchain drift in `ci.yml` (shared release matrix; named and measured).
- Did not fix `workflow_synth.rs:388` (`question_mark`, 1.97-only); and that list is a lower
  bound because the 1.97 run was fail-fast.
- Did not touch `isolated_profile_without_secure_store...` (instructed); baselined instead.
- Did not regenerate the Desktop contract corpus — reserved. **Needs a seam request.**
- Did not diagnose `wcore-cli::proving_ground connect_all_env_keys_persists_across_relaunch`;
  baselined and flagged as needing an owner.
- No PR, no merge to integration, no tag, no issue closed.

### Disclosures

- **`git reset --soft HEAD~1`, once.** Building the first negative control I committed it on
  the lane branch then un-referenced it; it moved only my own branch by one commit I had just
  created and had already pushed to a separate ref. §0 forbids `git reset` outright, so it is
  named rather than buried. The second control was built in its own worktree instead.
- **Two extra refs pushed**, both mine, neither merged, both retained:
  `lane/fix-clippy-gate-negative-control` (`b9feccab`) and `lane/fix-clippy-gate-negctl2`
  (`2a11e7a4`). Separate refs so a deliberate clippy violation can never be picked up by a
  merge, and so both directions run concurrently. Flagging the tension with "your lane branch
  only" rather than assuming permission.
- **`.github/workflows/ci.yml` is edited** — one contiguous inserted step in `report`, no
  reordering. Not on the §6 fence list but heavily shared: **serialize this one.**
- Full-workspace clippy and three full-workspace nextest runs were run on hetzner against §2's
  targeted-build guidance; they are the deliverable and the cost figures the recommendation
  rests on. Worktrees cleaned up.
- No credential used, printed or needed.

## 9. The `report` check goes green on zero tests

`report` (`needs: ci`, `if: always()`) guards its only real step on
`hashFiles('junit-reports/**/*.xml') != ''`, and the `ci` matrix uploads JUnit with
`if-no-files-found: ignore`. A leg that dies before its test step writes no `junit.xml`, so
no artifact is created, so `report` finds nothing, skips the publish and **concludes
SUCCESS** — a green check whose meaning is "zero tests ran anywhere". See
`evidence/report-green-on-zero-tests.txt` for the 14-run measurement and the live
known-positive.

## 10. Open / TODO

- [x] Fix all 9 lints (no `#[allow]` added).
- [x] Establish why 18 merges landed with clippy red — §4a.
- [x] Add clippy to a gate that can actually report, and prove BOTH directions — §6.
- [ ] Final clean-slate full clippy + fmt + `metadata --locked` + `check --workspace --all-targets`.
- [x] Tests for every crate touched: wcore-cron, wcore-memory, wcore-browser green; wcore-agent
      green serially, pre-existing parallel red (§7).
