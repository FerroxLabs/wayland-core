---
phase: 20-transactional-delegated-mutation
plan: "05"
subsystem: [agent, cli]
tags: [delegated-mutation, orchestration-wiring, production-spawner, anvil, council, crucible, workflow-cli, run-and-retain-seam]
requires: ["20-03", "20-04", "20-14"]
provides:
  - A public run-and-retain spawner seam (spawn_builder_into_retained_checkout) that runs a writing builder child in one transaction-owned isolated checkout and returns the still-armed MutationAttemptGuard without terminalizing
  - Anvil Forge/seat, Council, Crucible, and CLI workflow/anvil/crucible all routed through the workspace-aware production spawner — no orchestration caller can bypass classification or mutate the parent checkout
  - Per-candidate opaque transaction identity carried through Forge prompt/child/advisory-gate/reruns/BuiltCandidate with winner-only landing and RAII loser cleanup
affects: [20-07]
scope_expansion:
  from: 10
  to: 11
  added: [crates/wcore-agent/src/spawner/durable_launch.rs]
  rationale: "Task 1 (Forge) requires running a WRITING builder child in an isolated checkout AND retaining the checkout live for advisory gating + winner selection, but no public spawner seam did run-and-retain (execute_resolved_launch drops the retained workspace at child-end; mutation_attempt_guard retains but runs no child). Every in-scope alternative is plan-forbidden (process-global CWD / second nested checkout / read-only-can't-write). The orchestrator authorized adding up to spawner.rs + spawner/durable_launch.rs; the builder needed only durable_launch.rs (a descendant of the spawner module, reaching the private prepare_durable_launch/execute_durable_launch and ResolvedChildLaunch._transaction_workspace directly). spawner.rs is byte-identical to base. Precedent: 20-06/20-15 review-driven scope expansions."
key-files:
  created:
    - crates/wcore-agent/tests/anvil_forge_transaction.rs
  modified:
    - crates/wcore-agent/src/spawner/durable_launch.rs
    - crates/wcore-agent/src/orchestration/anvil/engine.rs
    - crates/wcore-agent/src/orchestration/anvil/forge.rs
    - crates/wcore-agent/src/orchestration/anvil/seat.rs
    - crates/wcore-agent/src/orchestration/council/run.rs
    - crates/wcore-agent/tests/common/mod.rs
    - crates/wcore-agent/tests/crucible_council.rs
    - crates/wcore-cli/src/anvil.rs
    - crates/wcore-cli/src/crucible.rs
    - crates/wcore-cli/src/workflow.rs
key-decisions:
  - "New public seam AgentSpawner::spawn_builder_into_retained_checkout(config, overrides, origin) -> Result<(SubAgentResult, MutationAttemptGuard), DurableSpawnerError>: fails closed BEFORE any child work if the request is not RequestedChildWorkspace::IsolatedMutation (never runs a mutating child in the parent checkout); allocates exactly one transaction-owned standalone checkout via the existing 20-04 prepare_durable_launch machinery; takes the retained TransactionWorkspace out of the launch by &mut BEFORE execute so the durable path's scope-exit drop no-ops for the checkout (liveness held across the whole child run); runs the builder bound to the checkout's realized workspace_root (never process CWD); returns the still-armed MutationAttemptGuard (the SAME type the 06C acceptance path consumes) instead of terminalizing. No second checkout, no global CWD, no bare-path identity. Preserves the audited 06A-06D invariants."
  - "Each SpawnBuilder::build candidate opens its OWN durable child transaction via the seam; Forge wraps the returned guard in a RetainedCheckout: CandidateCheckout (the candidate's opaque identity) carried through prompt (repo-relative, no path leaked) / child / advisory SandboxGate / stability reruns / returned BuiltCandidate. EvaluationGateExecutor::run re-derives the subject root through that identity (re-minting the candidate seal) on every call, so a substituted/stale/sibling checkout fails closed. run_climb retains only the selected winner in ClimbOutcome; every rejected/displaced loser terminalizes by RAII on drop; no candidate identity is reused or collapsed."
  - "Seats/Council/CLI propagate parent-workspace authority through 20-04's govern_standalone_spawner (which already binds with_parent_workspace(&cwd) internally), so the e200c0a explicit `workspace: &Path` param threading is now OBSOLETE/REJECTED; mutating builders are always isolation-eligible and read-only Council proposers still resolve their shared workspace."
  - "SCOPE-FORCED DEVIATION: dropped the 'session-seat retries the climb once' fallback in drive_climb_full — the retry reused the valve/session seat as a builder, but the isolated-mutation seam requires a concrete &AgentSpawner while the fixed out-of-scope tool.rs signature passes the valve only as &dyn Spawner. A driver-seat probe failure now reports `blocked` honestly (more correct than reusing a read-only seat as a builder). The valve read-only diagnostics are unchanged. ClimbOutcome/BuiltCandidate lost their Clone derives (they now own !Clone lifecycle identities); best_worktree: Option<PathBuf> retained as a display echo so out-of-scope tool.rs keeps compiling, with the real identity in the new winner field."
patterns-established:
  - "Orchestration mutation is obtainable only by running the child in a production-spawner-allocated transaction-owned isolated checkout whose retained guard is the candidate's opaque identity through gating and winner-only landing; non-isolated mutation fails closed before any child runs."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
source_sha: a528dbc77750f5812231d990ac1cd4ff4ba5146f
source_tree: e8b37680eb776bcfc74fda67a883a6c416312b38
task_base: 8321f8e25e6cf32476cd160a7543a73e26eb1e62
changed-paths:
  - crates/wcore-agent/src/spawner/durable_launch.rs
  - crates/wcore-agent/src/orchestration/anvil/engine.rs
  - crates/wcore-agent/src/orchestration/anvil/forge.rs
  - crates/wcore-agent/src/orchestration/anvil/seat.rs
  - crates/wcore-agent/src/orchestration/council/run.rs
  - crates/wcore-agent/tests/anvil_forge_transaction.rs
  - crates/wcore-agent/tests/common/mod.rs
  - crates/wcore-agent/tests/crucible_council.rs
  - crates/wcore-cli/src/anvil.rs
  - crates/wcore-cli/src/crucible.rs
  - crates/wcore-cli/src/workflow.rs
salvage:
  source: e200c0a178feb698af350312a80a33d5b04fc699
  rejected:
    - "anvil/seat.rs + cli/{anvil,crucible,workflow}.rs `workspace: &Path` param threading into govern_standalone_spawner — obsolete (20-04 binds with_parent_workspace internally)"
    - "every second-checkout / process-global-CWD Forge behavior"
  adapted:
    - "council/run.rs + tests/common/mod.rs + tests/crucible_council.rs `with_parent_workspace(dir)` + real create_for_run/create fixture pattern"
  constructed-fresh:
    - "Forge explicit-root transaction behavior (per plan)"
deferred:
  - "PRE-EXISTING (not 20-05): `spawn_tool::partial_failure_rollup_tests::spawn_batch_partial_failure_is_error` and `spawn_tool::durable_topology_origin_tests::requested_topology_is_preserved_in_the_durable_supervisor` fail DETERMINISTICALLY under nextest isolation with a 20-04-era `parent workspace authority is not bound` error. Proven pre-existing: spawn_tool.rs + spawner.rs are byte-identical base(8321f8e)→HEAD; identical failures on a clean base checkout; present in the 20-12/20-13 serial-failure sets. Must be resolved (likely a test-setup parent-workspace binding gap) before the 20-08 aggregate full-suite proof."
---

# Phase 20 Plan 05: Route All Orchestration Callers Through the Workspace-Aware Production Spawner

**Anvil Forge/seat, Council, Crucible, and the CLI workflow/anvil/crucible entry points now delegate exclusively through the production spawner. Each candidate runs in its own transaction-owned isolated checkout via a new run-and-retain seam; mutation without isolation fails closed. Linux-proven at source `a528dbc`. Admits 20-07.**

## Admission

- 20-14 pair re-proven green (`review-pair-ok` + `review-result-ok f20-14`).
- Task base `gsd-task-base-20-05` = `8321f8e` / tree `212d7849`.

## Scope expansion (10 → 11, authorized)

Task 1's Forge requires a public spawner seam that runs a writing builder in an isolated checkout AND retains that checkout for gating/selection — which did not exist (the run path terminalizes; the retain path never runs the child). Adding `spawner/durable_launch.rs` (a `spawner`-module descendant reaching the private machinery) supplied the seam without touching `spawner.rs` (byte-identical to base) and without any plan-forbidden workaround. Precedented by 20-06/20-15.

## Verification (Linux, committed-HEAD Hetzner harness, source `a528dbc`)

- **Scope:** `scope-ok base=8321f8e paths=11`; `git diff --check` clean; `fmt --all -- --check` clean.
- **`clippy -p wcore-agent -p wcore-cli --all-targets --all-features -- -D warnings`:** clean (a first run caught a real dead-code error — `BUILDER_TIMEOUT` unused after a refactor — fixed by re-applying the load-bearing per-fork wall-budget timeout around the seam call).
- **`test -p wcore-agent --test anvil_forge_transaction`:** 4 passed / 0 failed — real transaction-owned checkouts, on-disk winner retention + loser cleanup, parent-workspace-untouched, no-process-CWD, per-candidate identity distinctness.
- **`test -p wcore-agent --test crucible_council`:** 15 passed / 0 failed — cross-orchestrator isolation + `mutating_child_without_isolation_fails_closed`.
- **`test -p wcore-cli --lib`:** 1688 passed / 0 failed / 1 ignored — CLI authority + `…cannot_downgrade_mutation` denial tests.
- **`test -p wcore-agent --lib`:** cargo 2050/19; nextest process-isolation 1973/2. 17 of 19 are the documented pre-existing journal-lease parallelism flake (pass isolated). The **2 deterministic failures are PRE-EXISTING spawn_tool tests, not a 20-05 regression** — see deferred; **zero failures in any file 20-05 touched** (anvil/*, council/run, spawner/durable_launch).

## Read-only compatibility + fail-closed mutation

Council/Crucible proposers stay shared-read-only and resolve their workspace (proven). The seam refuses any non-`IsolatedMutation` request before running the child and never runs a writing child outside an isolated checkout (`mutating_child_without_isolation_fails_closed`, CLI `cannot_downgrade_mutation`) — no parent-checkout mutation, no child dispatched on a downgrade.

## Explicit non-claims

No parent landing / CAS (that is 20-07 — which consumes the winner's retained checkout from `ClimbOutcome`), no full lifecycle, no native-runtime, release, or deployment claim. Admits 20-07.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
