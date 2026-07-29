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

## Status log

- [x] Worktree verified, hetzner worktree created at base.
- [x] Full census (test vs production, + `#[serial]` attribution).
- [ ] Baseline failure-rate measurement (N reps at base).
- [ ] Fixes.
- [ ] After failure-rate measurement (same N, same box).
