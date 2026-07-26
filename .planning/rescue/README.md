# Rescued in-flight repair work — 2026-07-27

Five of the six Phase-21 product-repair agents were killed mid-edit by a **monthly spend
limit**, not by any problem with their work. Their worktrees had uncommitted changes; those
diffs are captured here as patches so nothing depends on the worktrees surviving.

| Patch | Finding | Contents |
|---|---|---|
| `wt-f21-02-01.patch` | Tool authority not intersected | `bootstrap.rs`, `engine.rs`, `spawner.rs` (agent + types), plus a NEW test `crates/wcore-cli/tests/f21_02_01_child_tool_authority.rs` |
| `wt-f21-02-03.patch` | PolicyGate unreachable (fail-open) | `policy_gate.rs`, `bootstrap.rs`, `engine.rs`, `spawner.rs`, `config.rs`, four test-harness files, `docs/getting-started.md`, plus a NEW test `crates/wcore-agent/tests/policy_gate_inheritance_test.rs` |
| `wt-f21-04-02.patch` | Reservation does not survive restart | `budget_authority.rs`, `child_attribution_corpus.rs`, `cases.rs` |

**These are UNVERIFIED.** None was compiled, tested, or reviewed — the agents died before
reaching their own proof steps. Do not apply any of them without running the full
red-before / green-after demonstration the repair brief required. Source worktrees, if still
present: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/wt-f21-02-0{1,3}` and
`wt-f21-04-02`, all based at `df63a4af`.

Apply with `git apply --3way .planning/rescue/<name>.patch` from the repo root.
