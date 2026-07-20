---
phase: 20-transactional-delegated-mutation
plan: "04"
subsystem: agent
tags: [delegated-mutation, spawner, bootstrap, engine, workspace-authority, production-wiring]
requires: ["20-01", "20-03", "20-15"]
provides:
  - Workspace-aware production spawner contract carrying explicit shared/isolated authority
  - Bootstrap/engine/Spawn-tool propagation of the parent repository identity and 20-01 transaction opening
  - Mutating children launch in transaction-owned standalone checkouts whose lifecycle terminalizes exactly once
  - Live production-composition regression tests for workspace-authority propagation and denials
affects: [20-06]
tech-stack:
  added: []
  patterns: [pre-allocation workspace classification, retained standalone-checkout lifecycle handle, fail-closed authority binding, production-composition denial tests]
key-files:
  created: []
  modified:
    - crates/wcore-agent/src/spawner.rs
    - crates/wcore-agent/src/durable_spawner.rs
    - crates/wcore-agent/src/bootstrap.rs
    - crates/wcore-agent/src/engine.rs
    - crates/wcore-agent/src/spawn_tool.rs
    - crates/wcore-agent/tests/workflow_live_gate_test.rs
key-decisions:
  - "Reuse the existing 20-01/20-03 abstractions (RequestedChildWorkspace, ForkOverrides::requested_workspace, ResolvedChildLaunch, resolve_durable_launch_in_workspace, WorktreeManager::create_isolated_checkout, delegated_mutation WorkspacePolicy) — no new token, intent enum, classifier, or workspace abstraction."
  - "Record one pre-allocation workspace classification and carry it through durable opening, allocation, launch, and terminal diagnostics; no production path synthesizes a default after dispatch begins."
  - "Retain the isolated child's TransactionWorkspace as a !Clone RAII lifecycle handle carried through ResolvedChildLaunch; cleanup terminalizes exactly once (released AtomicBool) at execute_resolved_launch scope-exit after engine.run; any pre-return failure drops the handle and rolls back."
  - "Bind the parent repository identity at bootstrap AND at every transient-spawner construction; when no contained WorkspacePolicy exists, bind from the session workspace root (config.session.directory), fail-closed for an unresolvable/relative root — never a process-global cwd for children."
  - "Delete the legacy global-cwd default_child_workspace fallback and the ChildWorkspaceMode::External path."
patterns-established:
  - "Every production spawn is classified read-only/shared or mutating/isolated before allocation."
  - "A mutating child owns one standalone checkout with independent Git metadata, and its cleanup handle terminalizes exactly once."
  - "Invalid, unbound, stale, rebound, or mismatched workspace authority stops before provider/tool execution with preserved diagnostics."
requirements-completed: []
duration: n/a
completed: 2026-07-20
status: complete
source_sha: 30a2b4f11345561e80d4f39f3b6bae1a6a50164c
source_tree: 4ab34b9170b4116541ab61d5cdb36dfff515942f
task_base: 2e26698f8a3d50117c45404581227f844a912077
coverage:
  - id: SC1
    description: "Every production spawn is classified; mutating never collapses to shared; unbound/stale/mismatched authority stops before execution."
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-04-live ... test -p wcore-agent --test workflow_live_gate_test (12 passed, incl. classification, unbound-authority fail-close, 7-field EvidenceMismatch rebind, substituted-workspace refusal)"
        status: pass
      - kind: unit
        ref: "remote-cargo.sh f20-04-spawner ... test -p wcore-agent --lib spawner (68 passed, incl. production_durable_spawn_tests quota/near-cap)"
        status: pass
  - id: SC2
    description: "Mutating children launch only in transaction-owned standalone checkouts with independent Git metadata; lifecycle terminalizes exactly once."
    verification:
      - kind: unit
        ref: "spawner lib tests + retained-lifecycle threading (create_isolated_checkout -> TransactionWorkspace RAII handle)"
        status: pass
  - id: SC3
    description: "The real workflow gate and strict Clippy pass remotely."
    verification:
      - kind: other
        ref: "remote-cargo.sh f20-04-clippy ... clippy -p wcore-agent --all-targets --all-features -- -D warnings"
        status: pass
---

# Phase 20 Plan 04: Production Spawner Workspace-Authority Propagation

**The Phase 20 isolation substrate is now wired through the real agent bootstrap, engine, Spawn tool, and durable spawner: every production spawn is classified, mutating children launch in transaction-owned standalone checkouts that terminalize exactly once, and invalid authority fails closed before execution — proven on Linux.**

## Performance

- **Completed:** 2026-07-20
- **Tasks:** 3 (contract, wiring, denial tests) + 3 remote-caught fix rounds
- **Files modified:** 6 (all `wcore-agent`)
- **Integrated source:** `30a2b4f` — tree `4ab34b9` (task base `2e26698`)

## Accomplishments

- Extended the spawner contract to carry an explicit read-only/shared vs mutating/isolated decision, recorded once before allocation and traced through durable opening, allocation, launch, and terminal diagnostics; deleted the legacy global-cwd `default_child_workspace` fallback and the `ChildWorkspaceMode::External` path.
- Wired bootstrap (parent identity bound on every production spawner), the engine (transient spawners bound to the session workspace; the engine never rewrites a mutating child's cwd back to the parent), and the Spawn tool (locked to shared read-only via `ForkOverrides::requested_workspace`).
- Retained the isolated child's `TransactionWorkspace` as a `!Clone` RAII lifecycle handle through `ResolvedChildLaunch`, terminalizing cleanup exactly once at child end (via the `released` AtomicBool) with rollback on any pre-return failure.
- Added live production-composition regression tests exercising the real `AgentSpawner`: classification, unbound/unavailable-authority fail-close, 7-field `EvidenceMismatch` rebind refusal, substituted-workspace refusal without a journal write, and parent-mutation absence.

## Task Commits

1. **Task 1: spawner contract carries explicit workspace authority** — `2afecb3`
2. **Task 2: wire bootstrap, engine, Spawn tool to the authority contract** — `0e51a12`
3. **Task 3: prove production-path propagation and denials** — `d5b9f9a`
4. **Fix: retain TransactionWorkspace lifecycle + correct create_isolated_checkout call** — `98bdc96`
5. **Fix: extract denial errors without requiring Debug on spawner types** — `1c54d52`
6. **Fix: bind transient spawner to session workspace + reconcile quota accounting** — `30a2b4f`

## Verification

All plan-level gates ran green on the committed-head Hetzner harness at the exact source `30a2b4f`:

- **`test -p wcore-agent --lib spawner`:** 68 passed, 0 failed.
- **`test -p wcore-agent --test workflow_live_gate_test`:** 12 passed, 0 failed.
- **`clippy -p wcore-agent --all-targets --all-features -- -D warnings`:** Finished clean.
- **Per-task construction gates** (`vx cargo fmt`, `git diff --check`, `verify-task-scope.sh … paths=6`): pass.

## Remote-caught defects (the reason construction commits are not evidence)

The Mac never compiles this repo; the remote gates surfaced defects that construction alone hid:
- **Compile (2 rounds):** Task 1 called `create_isolated_checkout` with the wrong arity and treated its `TransactionWorkspace` return as a path — which also masked a design gap (dropping the handle would delete the checkout before launch). Fixed by supplying `WorkspaceCapacity`, using `workspace.checkout`, and threading the retained lifecycle handle. Task 3's tests needed `Debug`-free error extraction.
- **Runtime (2 real bugs):** (a) `govern_transient_spawner` left the fresh production transient spawner **unbound** when no contained `WorkspacePolicy` existed, so every workflow-synthesis/crucible spawn would fail in a non-contained session — fixed to bind from `config.session.directory`, fail-closed for an unresolvable root. (b) The spawner's `retained_workspace_allocation_count` counted the manager's `.wayland-control` dir, an off-by-one that rejected one workspace too early near the cap — fixed to skip the control dir, mirroring the canonical `retained_worker_count`.

## Next Phase Readiness

Plan 20-06 depends directly on this accepted 20-04 integration successor (`30a2b4f`) and builds the 20-06A candidate-seal packet on it, reusing the standalone-checkout capability. This plan does not by itself mark F20-01/F20-02 complete. Native Windows/macOS execution remains deferred to plan 20-08. Unlike 20-03, plan 20-04 carries no dedicated independent-review gate (the review gates sit at the candidate-seal boundaries 20-09/20-11/20-16 and the 20-14 audit); its acceptance rests on the green committed-head Linux gates above, the production-composition hostile tests, and staged per-task cross-audit.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
