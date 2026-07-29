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

## Status log

- [x] Worktree verified, hetzner worktree created at base.
- [ ] Full census (test vs production classification).
- [ ] Baseline failure-rate measurement (N reps at base).
- [ ] Fixes.
- [ ] After failure-rate measurement (same N, same box).
