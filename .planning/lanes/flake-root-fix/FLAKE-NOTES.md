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

## *** HIGH — REAL DEFECT, NOT CONTENTION: the test binary SIGSEGVs ***

`/tmp/flake-root-fix-base-config/rep-8.log`, at BASE `eaff921d`, unmodified tree:

```
error: test failed, to rerun pass `-p wcore-config --lib`
Caused by:
  process didn't exit successfully:
  `/root/wayland-flake/target/debug/deps/wcore_config-9fe3fef9b4bf10f1`
  (signal: 11, SIGSEGV: invalid memory reference)
```

Rate at base: **1 crash in the first 9 reps of `wcore-config --lib`** (~11%). Zero
crashes in 100 reps across exec-backend (x2) and migrate_hermes (x2) — it is specific
to the crate with by far the heaviest concurrent env mutation (config.rs alone has 78
sites).

**This is memory-unsafety, not a flaky assertion.** glibc's `setenv`/`unsetenv` are NOT
thread-safe: `setenv` can `realloc` the `environ` array while another thread is inside
`getenv`, which then dereferences freed memory. This is exactly why Rust made
`std::env::set_var` `unsafe` in edition 2024 — and every site in this codebase silences
it with an `unsafe` block plus a comment asserting single-threadedness that is FALSE
under `cargo test`.

**Consequence for the whole flake story: `#[serial]` does NOT close this.** Serial
groups serialize WRITERS against each other, but a writer still runs concurrently with
every READER — any test calling `Config::resolve`, `std::env::var`, or any library that
reads env. The UB window stays open. So the `#[serial]` repairs below fix the *logical*
races (wrong values) and reduce, but do not eliminate, the crash surface. Fully closing
it means not mutating process env in a multi-threaded process at all — i.e. injection
everywhere, which is a much larger change than this lane. **Reported, not papered over.**

## BASELINE — `wcore-config --lib`, 25 reps at base `eaff921d`

Recomputed from the 25 per-rep logs on disk (the harness's own summary line was
lost: I overwrote the running script with `scp`, and bash re-reads a script from
its byte offset, so the in-flight run died with a syntax error at rep ~24. The
per-rep logs were already written and are unaffected; tally is from them.)

| | count | of 25 |
|---|---|---|
| PASS | 9 | 36% |
| FAIL (assertion) | 14 | 56% |
| **CRASH (SIGSEGV)** | **2** | **8%** |
| **failure rate (FAIL+CRASH)** | **16** | **64%** |

**The victim set MOVES between runs**, exactly as the four lanes reported — five
different tests failed across the 25 reps:

| failing test | reps |
|---|---|
| `config::tests::resolves_same_and_cross_provider_fallbacks_with_independent_credentials` | 9 |
| `env_file::tests::load_wayland_env_file_applies_without_overriding` | 2 |
| `config::tests::test_resolve_with_project_dir_loads_project_config` | 2 |
| `config::tests::test_resolve_omitted_max_tokens_reads_as_not_explicit` | 1 |
| `config::tests::test_resolve_cli_max_tokens_marks_explicit` | 1 |

Every one of these is a VICTIM (a reader), not a source. That is why four lanes
each diagnosed a different "root cause" — they were each looking at whichever
reader lost the race that run.

## CENSUS — final numbers (repaired scanner; see instrument repairs below)

Contention unit is the TEST BINARY (`cargo test` gives each crate's `--lib` and
each `tests/*.rs` its own process), and only variables touched by >1 test in the
SAME binary can flake.

| | base `eaff921d` | fixed |
|---|---|---|
| UNPROTECTED mutators of a contended var | **7** | **0** |
| Regime splits (1 var, >1 independent lock, same binary) | **4** | **0** |

The 7: `wcore-browser::lib` x2 (`WAYLAND_CAMOUFOX_BIN`), `wcore-config::lib` x4
(`WAYLAND_HOME`, `XDG_DATA_HOME`, `ANTHROPIC_API_KEY`), `wcore-cua::lib` x1
(`WAYLAND_DISPLAY`).

**The "regime split" class is the subtle one and was not previously reported.**
`serial_test`'s groups are INDEPENDENT locks: a bare `#[serial]` takes the
default group and does NOT exclude `#[serial(wayland_home_env)]`. In
`wcore-config::lib`, `WAYLAND_HOME` was mutated from THREE regimes at once
(UNPROTECTED, default, `wayland_home_env`). A test can therefore look correctly
protected and still race.

## FALSE PREMISES IN THE SOURCE (each one a comment asserting safety that was untrue)

1. `wcore-exec-backend/src/registry.rs:119` — "single-threaded per process under
   nextest". True for CI's runner, false for `cargo test`.
2. `wcore-config/src/config.rs` (`wayland_config_dir_uses_wayland_home_when_set`)
   — "Serial isolation is not required here because we restore the env var
   within the test; the variable name is unique to this assertion." Restoring
   does nothing for the set→restore WINDOW, and `WAYLAND_HOME` has 141
   references.
3. `wcore-config/src/config.rs` (`test_api_key_missing_returns_error`) —
   "single-threaded test context; no other threads read these vars."
4. `wcore-config/src/env_file.rs` — "#[serial] keeps it off the other
   env-reading tests' threads." It was in the DEFAULT group while all ~14 peers
   were in `wayland_home_env`, so it excluded none of them.
5. `wcore-browser/src/liveness.rs` — "the only env var touched is this one, and
   no other test in this module reads it." The very next test in that module
   sets the same variable.
6. `wcore-cua/src/liveness.rs` — "no other test in this module touches them."
   Scoped to the MODULE; the contention unit is the BINARY, and `adapter.rs`
   mutates the same variable.

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
