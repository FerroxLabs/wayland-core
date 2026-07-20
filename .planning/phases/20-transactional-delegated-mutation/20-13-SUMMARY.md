---
phase: 20-transactional-delegated-mutation
plan: "13"
subsystem: [agent, sandbox]
tags: [delegated-mutation, black-box-proof, hard-containment, gate-execution, accepted-candidate, 06d]
requires: ["20-12"]
provides:
  - Live-backend hard-containment qualification + descendant no-residue proof (black-box, sandbox public API)
  - Hostile gate-evidence + durable-replay fail-closed proof and the positive host-candidate → AcceptedCandidate proof (black-box, agent public API)
  - A committed-HEAD proof set (8 focused pairs + integration + clippy + fmt, all Linux-green) over the exact integrated 06C/06D candidate, ready for the fresh 20-14 audit
affects: [20-14]
key-files:
  created:
    - crates/wcore-sandbox/tests/hard_process_containment.rs
    - crates/wcore-agent/tests/child_transaction_gate_test.rs
    - crates/wcore-agent/tests/child_transaction_gate_execution_test.rs
  modified: []
key-decisions:
  - "06D adds only the three declared black-box test files (scope-ok paths=3) through PUBLIC production surfaces (run_gate_acceptance / AcceptedCandidate / establish_hard_containment); NO production path is authored, so no 06C receipt is invalidated and the 06D source→summary chain is metadata-only."
  - "The live containment proof runs a real semantically-probed bubblewrap PID-namespace backend: it mints a one-use HardContainmentAuthority, refuses on spawn-parameter drift, and proves descendant no-residue by reaping a detached `sleep 45` grandchild the instant the namespace init exits (a falsifiable bound well below the 45s sleep / 30s timeout) on both zero- and nonzero-exit terminal paths."
  - "The positive path reaches an opaque guard-owned AcceptedCandidate ONLY through run_gate_acceptance after the authoritative durable append/reopen/reduce/match; hostile evidence (missing/substituted/unknown-gate/subject-drift/model-claimed/malformed/duplicate/conflicting/corrupted-journal/copied-journal-rebind) fails closed with no durable effect; abnormal termination (cancel/timeout/process-death/dropped-guard/restart) terminalizes+cleans exactly once via the shared RAII guard with no parent mutation."
  - "DEVIATION (surfaced to 20-14): a black-box caller cannot author a non-empty ChildGatePlan because the gate_closure_digest is produced by the crate-private AuthorizedGateClosureRegistry, so the positive test drives the full acceptance PIPELINE over a gate-less plan; live per-gate containment execution is proven separately through the sandbox public API in hard_process_containment.rs, and hostile order/count/duplicate enforcement (crate-private AcceptanceMachine, proven by 20-12's inline stage test) is proven at the durable-receipt validation those shapes reduce through."
  - "windows-gnu cross-check surfaced a PRE-06D regression (root-caused below) in wcore-sandbox windows_impl — production code OUTSIDE 06D's test-only scope and OUTSIDE the 06A-06D audited blob set. It is routed to 20-08 (the windows_impl owner) with a validated fix patch; per profile f20-14 native Windows is a deferred check, so it does not block the 20-14 audit of the Linux-provable 06A-06D core."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
source_sha: ace4bd26fa3d831b2129ce319248652dbc25f5b7
source_tree: 25c2c6c8b5d5d6eed7c33fc8e89c1c98619e2c5d
task_base: e19cf4ed7569648ddded9dd20ff116ed7954daa8
qualification_pairs:
  f20-09: { source_sha: 10d75737a42b0d6b9aeaa42f1dea9fb06e5613c7, review_sha: 8d66277115f0fe3cd9b9a2d559707ba8b9bd9775 }
  f20-11: { source_sha: b1de890363ab82ba952ad03bb5e692461c1cc8b5, review_sha: 5cd67c5a152a912ae55fc7c43bc88cded02b4944 }
changed-paths:
  - crates/wcore-sandbox/tests/hard_process_containment.rs
  - crates/wcore-agent/tests/child_transaction_gate_test.rs
  - crates/wcore-agent/tests/child_transaction_gate_execution_test.rs
repair_invalidation_rule: "Any 06D source repair reruns the six inherited 20-12 pairs plus both final pairs and every aggregate gate on the repaired exact commit before 20-14 may proceed."
deferred:
  - "native Windows (per f20-14): the wcore-sandbox windows_impl cross-compile regression (below) is owned by 20-08; validated fix staged as scratchpad/windows-impl-modpath-fix-for-2008.patch. Full validation at the 20-17/20-18 native-Windows UAT."
---

# Phase 20 Plan 13: Black-box Gate-Execution + Hard-Containment Proofs (06D)

**The private 06C gate/acceptance construction is now falsifiable behavior: a live qualifying backend owns the accepted process tree, hostile evidence fails closed, and the positive path reaches AcceptedCandidate only through authoritative durable replay. 06D source `ace4bd2`; all owned gates Linux-green. Makes NO landing claim. Admits 20-14.**

## Admission

- f20-09 (06A) and f20-11 (06B) qualification pairs re-proven green (`review-pair-ok` both).
- Task base `gsd-task-base-20-13` = `e19cf4e` / tree `5369505`.

## Verification (Linux, committed-HEAD Hetzner harness, source `ace4bd2`)

- **Scope:** `scope-ok base=e19cf4e paths=3` — exactly the 3 declared NEW test files, zero production source.
- **Eight focused list/run pairs (process-isolated nextest):** each 1 nonignored selected / 1 un-retried PASS — the six inherited 20-12 pairs plus **`qualified_hard_containment_backend_preflight`** and **`qualified_host_candidate_to_accepted_candidate`**.
- **Integration binaries:** `child_transaction_gate_test` 7/0; `child_transaction_gate_execution_test` 1/0; `hard_process_containment` 1/0.
- **Aggregate:** `test -p wcore-swarm --lib` 92/0; `test -p wcore-sandbox --lib` 80/0; `clippy -p wcore-swarm -p wcore-sandbox -p wcore-agent --all-targets --all-features -- -D warnings` clean; `fmt --all -- --check` clean.
- **`test -p wcore-agent --lib`:** the SAME pre-existing non-deterministic journal-lease + inherent-parallelism contention documented in 20-12 (failures only in `engine`/`session`/`orchestration::council::run`/`spawn_tool` — none in `child_transaction`; the 06D tests live in integration binaries, not `--lib`). Not a 20-13 regression.

## windows-gnu gate: surfaced a PRE-06D regression (root-caused, routed to 20-08, NOT a 06D defect)

`check -p wcore-sandbox --target x86_64-pc-windows-gnu` was RED on the candidate (4 errors in `backends/appcontainer/windows_impl/{handles,process}.rs`). The orchestrator bisected it via parallel worktrees:

| Commit | windows-gnu |
|--------|-------------|
| Phase-20 entry `97e4491` | **Finished clean** |
| pre-06B `1ace696` | 4 errors |
| 06D `ace4bd2` | 4 errors |

**Root cause:** `4dcd62a` (plan **20-02**, "isolate AppContainer execution identities") moved `windows_impl` one module level deeper (under `backends/appcontainer/`) but left the `super::super::` relative paths to `reserve_output` / `BUFFERED_OUTPUT_LIMIT_BYTES` (in `backends`) and `probe_single_flight` (in `appcontainer`), plus two sibling-module private-field constructions (`SharedJob`, `AttrListGuard`). Because `windows_impl` is `#[cfg(windows)]` — dead on Linux — every Linux-only Phase-20 proof, INCLUDING 20-02's own acceptance, was blind to it. 20-13's new windows-gnu cross-check is the first thing to compile that path and surface it.

**Disposition:** this is production `windows_impl` code — OUTSIDE 06D's test-only scope, OUTSIDE the 06A-06D audited blob set (the 20-14 verify-review-pair blob list excludes `windows_impl`), and it must NOT enter the 06D→summary metadata-only chain. It is routed to **20-08** (the designated `windows_impl` owner; its scope already covers `windows_impl #![allow(unused_imports)]` cleanup). A **validated fix** — corrected relative paths (`+1 super`) + `pub(super) fn new` constructors for `SharedJob`/`AttrListGuard`, `#[cfg(windows)]`-only and Linux-neutral — was proven green out-of-chain (windows-gnu `Finished`, 0 errors) and staged as `scratchpad/windows-impl-modpath-fix-for-2008.patch`. Per profile **f20-14, native Windows is a deferred check**, so this does not block the 20-14 audit of the Linux-provable 06A-06D core; it is fully validated at the 20-17/20-18 native-Windows UAT.

**Systemic note for the roadmap:** all Phase-20 proofs are Linux-only, so `#[cfg(windows)]`/`#[cfg(macos)]` code can regress invisibly until a cross-target gate compiles it. The windows-gnu cross-check should run earlier in the gate sequence (it first appears at 20-13). Flagged for the roadmap and for 20-08.

## Deviations / limitations (surfaced to 20-14)

- **Gate-less positive plan** (see key-decisions): the black-box positive test cannot author a gated durable plan without a production change; live per-gate containment is proven separately via the sandbox public API. The 20-14 auditor should read both agent test module headers.
- **windows_impl cross-compile regression** — pre-06D, out-of-scope, routed to 20-08 with a validated patch; Windows deferred under f20-14 (above).
- **No native Windows runtime claimed** — the Linux containment + git-checkout tests are Linux-gated.

## Explicit non-claims

No parent landing, full lifecycle, parent-CAS, release, or native-Windows-runtime claim. Acceptance proves observed-execution + authoritative durable replay + guard-owned terminalization only. Admits the fresh non-author 20-14 all-severity audit of the unchanged integrated 06A-06D candidate.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
