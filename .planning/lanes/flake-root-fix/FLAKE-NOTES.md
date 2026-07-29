# NOTES — lane/flake-root-fix

Base: `eaff921d710876e87372f01dcce3b185004426bc` (plan/f20-unified-audit-repair)
hetzner worktree: `/root/wayland-flake` @ `eaff921d` (branch `hz/flake-root-fix`)
Started 2026-07-29.

## Mission

Four lanes independently re-diagnosed the same flaky-test family. Three root causes were
each independently found:

1. `crates/wcore-config/src/config.rs:319` `pub profiles: HashMap<..>` — iteration order
   reshuffles on write; `migrate_hermes::import_is_idempotent_without_overwrite` asserts
   byte equality across round-trips. Reported ~60% fail (10/25 pass in 25 reps).
2. `wcore-exec-backend` `registry::tests::a_recorded_task_is_readable...` — process-global
   env var in `with_temp_state`. ~1-in-3 under `cargo test`, 3/3 in isolation.
3. `wcore-config` `config::tests::test_resolve_cli_max_tokens_marks_explicit` and
   `..._without_project_dir_uses_cwd` — `std::env::set_var`/`remove_var`. 2 fail parallel,
   567/567 single-threaded.

Pattern: tests mutating process-global state, or asserting on HashMap iteration order.
nextest (process-per-test) hides it; `cargo test` (shared process) exposes it.

## Instrument liveness (per LANE-BRIEF §3b-i)

Known-positive probe in same invocation as the census grep:
`/usr/bin/grep -rn "fn main" --include="*.rs" crates/ | wc -l` -> **75** (non-zero, alive).
Census grep: `/usr/bin/grep -rn "set_var\|remove_var" --include="*.rs" crates/ | wc -l`
-> **758** raw hits. All greps unproxied (`/usr/bin/grep`).

## CENSUS (measured, unproxied, `census.py` + `census2.py` in this dir)

### Process-global mutation
- 743 total `set_var`/`remove_var` sites; TEST=713, PROD=30.
- 2 `set_current_dir` sites, both TEST (`wcore-config/tests/config_resolution_provenance.rs`).
- Refined by enclosing fn + `#[serial]` attribution (`census2.py`):
  - **UNPROTECTED (a `#[test]` fn mutating process globals with NO `#[serial]`): 61 sites / 14 files**
  - PROTECTED (`#[serial]`) mutation sites: 507
  - Mutations in non-test helper fns: 177 (these inherit their CALLER's protection,
    so they are only safe if every caller is `#[serial]` — `with_temp_state` is not)
- `#[serial]` appears 373 times workspace-wide. The machinery EXISTS; the flakes are
  the tests that skip it. **`#[serial]` only serializes against other `#[serial]` tests** —
  one unprotected mutator defeats every serial test sharing that variable.

### Most-contended keys (unproxied grep, name frequency)
`WAYLAND_HOME` 141, `HOME` 28, `OPENAI_API_KEY` 26, `CODEX_HOME` 24,
`GOOGLE_CLIENT_ID` 23, `ANTHROPIC_API_KEY` 19, `WAYLAND_EXEC_BACKEND_STATE_DIR` 5.
`WAYLAND_HOME` is the dominant collision surface by a factor of 5.

### Iteration-order surface
`ConfigFile` holds serialized `HashMap`s at config.rs:321 (`providers`) and :324
(`profiles`) — the brief named only `profiles`; `providers` has the identical defect.
Also `config.rs:172` `servers`, `:115` `env`, `:119` `headers`,
`mcp_cred_refs.rs:165` `connectable`.

## MECHANISM — confirmed, and it splits into TWO different kinds

**Kind A — genuine PRODUCT nondeterminism (not a test problem at all).**
`migrate_hermes::import_is_idempotent_without_overwrite` is **already `#[serial]`**.
So its ~60% failure is NOT contention. Two successive serializations of the same
logical config emit different TOML key order, because `RandomState` seeds each new
`HashMap` differently within a single thread. Consequence beyond the test: **the
product rewrites the user's `config.toml` with randomly reordered sections on every
save.** That is a real user-facing defect (spurious diffs, config churn) that the
flaky test was reporting correctly. `--test-threads=1` does NOT fix this one.

**Kind B — test-isolation leaks (contention).**
`wcore-exec-backend/src/registry.rs:117` `with_temp_state` carries the comment
"SAFETY-of-intent: these tests are single-threaded per process under nextest, which
runs each test in its own process." The assumption is explicit and is TRUE under the
CI runner (nextest) and FALSE under `cargo test`. It also leaks: the var outlives the
`TempDir`, so later tests in the process inherit a path to a deleted directory.
`wcore-config::config::tests::test_resolve_cli_max_tokens_marks_explicit` sets NO env
var of its own — it is a pure VICTIM of another test's leak via `Config::resolve`
reading process env.

## HARNESS SELF-TEST (`repeat.sh`, per LANE-BRIEF §6b-ii — three assertions)

A rep counts as PASS only if it BOTH exited 0 AND reported >= MIN_TESTS executed.

1. **Known-positive passes** — `-p wcore-config --lib test_resolve_cli_max_tokens_marks_explicit`
   -> `rep 1/1 rc=0 PASS(ran=1)`. (Also proves that test is GREEN IN ISOLATION,
   supporting the victim theory.)
2. **Known-negative fails** — demonstrated on real data by the exec-backend
   baseline: `rc=101 FAIL(ran=88)` graded FAIL, 18 times.
3. **The naive matcher would have missed it** — `-p wcore-config --lib
   zzz_no_such_test_name_xyz` -> `rep 1/1 rc=0 VACUOUS(ran=0)`. Exit status says
   SUCCESS; the harness grades VACUOUS. An exit-status-only matcher scores this a
   PASS. This is the assertion that proves the anti-vacuity check does anything.

## FIX 1 — wcore-exec-backend state dir (Kind B) — PROVEN

`registry.rs`: `state_dir()` now consults a **thread-local** override installed by an
RAII `StateDirGuard`, before the env var and the default. `with_temp_state` uses the
guard instead of `set_var`. Per-thread injection (the codebase's stated preference)
makes the tests independent WITHOUT serializing them and without weakening an assertion.
Injected at the single centralized resolver rather than threading a parameter through
its 11 call sites, which would have touched production paths in `local.rs`,
`container.rs`, `node/registry.rs` and `pairing.rs` for a test-isolation defect.

| | reps | PASS | FAIL | VACUOUS | rate | load avg |
|---|---|---|---|---|---|---|
| BEFORE `b6e4019f^` | 25 | 7 | 18 | 0 | **18/25 = 72%** | 1.69 |
| AFTER `b6e4019f`   | 25 | 25 | 0 | 0 | **0/25 = 0%** | 7.50 |

Single culprit in all 18 failing reps:
`registry::tests::a_recorded_task_is_readable_by_another_caller_and_removable`.
Raw output carried `0 ignored; 0 filtered out`, and `ran=89` on every green rep.
**The after-run was at 4x the load of the before-run** — for a logic race that makes
failure MORE likely, so 0/25 is a conservative result, not a lucky quiet box.

`tests/fail_closed_matrix.rs:559` also sets this var but is the ONLY mutation in its
binary, and integration-test binaries are separate processes, so it cannot race.
Left unchanged (AGENTS.md §3, surgical changes).

## NOT A DEFECT — do not "fix"

`always_fails` (`crates/wcore-cli/src/plugin/scaffold.rs:274`) is a **string literal the
product writes** into scaffolded plugin templates: `#[test] fn always_fails() {
panic!("deliberate"); }`. A scaffolded crate gets picked up as a workspace member, so its
deliberate panic surfaces in `cargo test -p wcore-cli --lib`. It is a FIXTURE and is
SUPPOSED to fail. At least six prior lanes have re-diagnosed it. Untouched.

## Status log

- [x] Worktree verified, hetzner worktree created at base.
- [x] Full census (test vs production, + `#[serial]` attribution).
- [x] Harness self-test (3 assertions).
- [x] Baseline + after for wcore-exec-backend: 18/25 -> 0/25.
- [ ] Kind A HashMap ordering fix + rate.
- [ ] wcore-config --lib baseline + after.
