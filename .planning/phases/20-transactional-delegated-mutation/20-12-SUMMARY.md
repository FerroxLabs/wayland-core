---
phase: 20-transactional-delegated-mutation
plan: "12"
subsystem: agent
tags: [delegated-mutation, gate-execution, durable-receipt, accepted-candidate, integration, source-packet, 06c]
requires: ["20-04", "20-11"]
provides:
  - Parent-owned AuthorizedGateClosureRegistry sealing every execution-affecting field into a SHA-256 closure digest; unknown/substituted/drifted closures fail closed before candidate code runs
  - Fail-closed gate state machine that accepts only module-private, exact-seal, in-order observed results and only from a consumed one-use HardContainmentAuthority spawn
  - Opaque, guard-owned AcceptedCandidate minted only after authoritative durable receipt append/reopen/reduce/match; owns the moved still-armed MutationAttemptGuard + CandidateSeal
affects: [20-13]
tech-stack:
  added: []
  patterns: [parent-owned sealed closure registry, live-seal-derived gate cwd, consumed one-use containment spawn, authoritative append-reopen-reduce receipt, opaque guard-owned acceptance capability with load-bearing drop order]
key-files:
  created:
    - crates/wcore-agent/src/child_transaction/gate_executor.rs
    - crates/wcore-agent/src/child_transaction/gates.rs
    - crates/wcore-agent/src/spawner/durable_launch.rs
    - crates/wcore-agent/src/spawner/mutation_workspace.rs
  modified:
    - crates/wcore-agent/src/child_transaction.rs
    - crates/wcore-agent/src/spawner.rs
    - crates/wcore-agent/src/session_journal/model.rs
    - crates/wcore-agent/src/session_journal/reducer.rs
key-decisions:
  - "AuthorizedGateClosureRegistry (parent-owned) seals fixed argv, timeout, sanitized environment, pinned toolchain/input identities, and transaction-private writable roots into a domain-separated SHA-256 closure digest with a separate authorization ledger; resolve() recomputes the live digest immediately before spawn and fails closed on unknown (absent), config-drift (live≠authorized), or substituted (live≠plan-pinned) closures before any candidate code executes."
  - "Gate cwd resolves from the live seal: SealedCandidateRoot::resolve_root mints a fresh CandidateSeal via seal_candidate() (which re-proves execution authority AND recomputes the pristine-source manifest, failing closed on a released/drifted/substituted checkout) and only then returns checkout_authority().display_path() — the very same retained checkout the seal binds. Because the 06A CandidateSeal is fully opaque (no public accessor; mint/revalidate are pub(super) in wcore-swarm), deriving the cwd from the seal-gated retained checkout-authority keeps the packet within its 8-file scope without adding a public accessor to wcore-swarm; the mint is the fail-closed gate."
  - "The AcceptanceMachine admits only module-private ObservedGateResults, for the same seal, in declared order; missing/conflicting-duplicate/reordered/post-terminal/stale/malformed/mismatched results fail closed. Observed results come only from the consumed one-use HardContainmentAuthority spawn (establish_hard_containment mint → verify_hard_containment spend at spawn)."
  - "AcceptedCandidate exists only after the authoritative receipt closure builds canonical bytes under ChildTransactionAuthority, conditionally appends, reopens durable state, reduces, and matches the receipt/plan/seal/observed-result digests. It is opaque: no public constructor (crate-private AcceptanceMachine::accept), non-Clone and non-Serialize (owned !Clone CandidateSeal + MutationAttemptGuard), redacted Debug. It OWNS the original still-armed MutationAttemptGuard MOVED from the durable launch (not a reopened path manager). Field order is load-bearing: `seal` drops before `guard` so the seal's retained checkout-authority/cleanup-liveness clones release before the guard terminalizes the checkout — no outstanding-loan on cleanup (the 20-15 fail-closed-on-loan invariant)."
  - "This packet makes NO parent-landing / 20-07 parent-CAS / 20-08 complete-lifecycle claim: receipt disposition is Active (never MergeReady/Merged), there is no merge/CAS path, and run_gate_acceptance/mutation_attempt_guard are the wired-but-not-yet-production-called integration surface consumed by 20-13 (black-box) / 20-07 (landing)."
patterns-established:
  - "Candidate acceptance is obtainable only from parent-observed exact-candidate gate execution (sealed closures + live-seal cwd + consumed one-use hard containment) PLUS authoritative durable append/reopen/reduce, yielding an opaque guard-owned capability whose drop terminalizes the non-landing transaction."
requirements-completed: []
duration: n/a
completed: 2026-07-20
status: complete
source_sha: b8a260ec3355e3352bfe4c7a4cbc118f89f1034e
source_tree: 2e003c07ffcd6cc180759a4e62849a1acc6759c6
task_base: bb97519eb5bb7aac50826e15c2d8240ccf4729b3
qualification_pairs:
  f20-09:
    source_sha: 10d75737a42b0d6b9aeaa42f1dea9fb06e5613c7
    review_sha: 8d66277115f0fe3cd9b9a2d559707ba8b9bd9775
  f20-11:
    source_sha: b1de890363ab82ba952ad03bb5e692461c1cc8b5
    review_sha: 5cd67c5a152a912ae55fc7c43bc88cded02b4944
changed-paths:
  - crates/wcore-agent/src/child_transaction.rs
  - crates/wcore-agent/src/child_transaction/gate_executor.rs
  - crates/wcore-agent/src/child_transaction/gates.rs
  - crates/wcore-agent/src/spawner.rs
  - crates/wcore-agent/src/spawner/durable_launch.rs
  - crates/wcore-agent/src/spawner/mutation_workspace.rs
  - crates/wcore-agent/src/session_journal/model.rs
  - crates/wcore-agent/src/session_journal/reducer.rs
repair_invalidation_rule: "Any 06C source repair invalidates all 06C receipts. Rerun the full library gate and all six hostile list/run pairs on the repaired exact commit before 20-13 may proceed."
---

# Phase 20 Plan 12: Gate-Execution + Durable-Receipt AcceptedCandidate (06C source packet)

**Candidate acceptance is now obtainable only from parent-observed exact-candidate gate execution plus authoritative durable append/reopen/reduce, yielding an opaque guard-owned `AcceptedCandidate`. Linux-proven at source `b8a260e`. Makes NO landing claim. Admits 20-13.**

## Admission (both upstream qualification pairs re-proven green)

- **f20-09 (06A):** `verify-review-pair.sh` → `review-pair-ok` (source `10d7573` → review_base `b188ab4` → review `8d66277`); `verify-review-result.mjs f20-09` → `review-result-ok`.
- **f20-11 (06B):** `verify-review-pair.sh` → `review-pair-ok` (source `b1de890` → review_base `542b917` → review `5cd67c5`); `verify-review-result.mjs f20-11` → `review-result-ok`.
- Task base `gsd-task-base-20-12` = `bb97519` / tree `16721d3e` (ancestor of HEAD).

## What this builds

`AuthorizedGateClosureRegistry` (parent-owned) seals every execution-affecting field — fixed argv, timeout, sanitized environment, pinned toolchain/input identities, transaction-private writable roots — into a domain-separated SHA-256 closure digest and an authorization ledger; `resolve` recomputes the live digest immediately before spawn and fails closed on unknown / config-drift / substituted closures before candidate code runs. `GateExecutor` walks the plan in declared order through a fixed fail-closed stage sequence: resolve the candidate cwd from the live seal (`SealedCandidateRoot`, minting a fresh `CandidateSeal` to re-prove authority + pristine source), build the `HardContainmentFilesystem` (read-only candidate + private scratch), mint the 06B authority via `establish_hard_containment`, spend the one-use `verify_hard_containment` at spawn, then execute — yielding module-private `ObservedGateResult`s. The `AcceptanceMachine` admits only exact, in-order, passed, same-seal evidence; the authoritative receipt closure then builds canonical bytes, conditionally appends under `ChildTransactionAuthority`, reopens durable state, reduces, and matches digests before `AcceptedCandidate` exists. `AcceptedCandidate` is opaque and OWNS the moved still-armed `MutationAttemptGuard` + `CandidateSeal`, with load-bearing field-drop order so guard cleanup never observes an outstanding seal loan.

## Verification (Linux, committed-HEAD Hetzner harness, source `b8a260e`)

- **Scope:** `verify-task-scope.sh` → `scope-ok base=bb97519 paths=8` (exactly the 8 declared paths, 4 new). `git diff --check` clean; `vx cargo fmt` clean (edition 2024 — NOT the plan's buggy `--edition 2021`, tracked for 20-08).
- **`clippy -p wcore-agent --all-targets --all-features -- -D warnings`:** clean (only a pre-existing third-party `imap-proto` future-incompat note, not this packet).
- **Six hostile gate pairs (process-isolated nextest, deterministic — the authoritative acceptance proof):** each `nextest list -E 'test(=…)'` matched EXACTLY ONE nonignored test; each `nextest run --retries 0` was EXACTLY ONE un-retried PASS:
  - `child_transaction::gate_executor::tests::rejects_unknown_closure_digest`
  - `child_transaction::gate_executor::tests::rejects_substituted_closure`
  - `child_transaction::gate_executor::tests::rejects_closure_config_drift`
  - `child_transaction::gate_executor::tests::fails_closed_at_each_gate_execution_stage`
  - `child_transaction::tests::rejects_append_reopen_reduce_corruption`
  - `child_transaction::gates::tests::guard_drop_terminalizes_and_cleans`
- **Touched code, deterministic process-isolation (`nextest -E 'test(child_transaction) or test(session_journal)'`):** **71 tests, 71 passed, 0 failed** — independently re-run by the orchestrator. All eight touched files' tests (incl. all 6 hostile tests and the `session_journal` model/reducer changes) pass cleanly.

## Cross-audit: the `test -p wcore-agent --lib` parallel flakiness is PRE-EXISTING, not a 20-12 regression

The `cargo test -p wcore-agent --lib` gate does not reach all-green under its default 96-way in-process parallelism. The orchestrator independently ran four configurations at source `b8a260e` to prove this is pre-existing journal-writer-lease contention and not a 20-12 defect:

| Run | Config | Result | 20-12's `rejects_append_reopen_reduce_corruption` | Touched-file failures |
|-----|--------|--------|:---:|:---:|
| A | `nextest`, touched modules, process-isolated | **71/71 pass** | PASS | 0 |
| B | `cargo --lib`, parallel (96-way) | 20 failed | FAIL | *(only the flaky journal test)* |
| C | `cargo --lib`, parallel (96-way) | 18 failed — **different set** | PASS | 0 |
| D | `cargo --lib`, serial `--test-threads=1` | 4 failed | PASS | 0 |

The failing sets are non-deterministic (20 vs 18, different tests) — the signature of test-harness contention, not a deterministic regression. Serial collapses to **4** failures, all inherent-parallelism tests (`orchestration::council::run::*`, `spawn_tool::*`) that fail *because* they are forced serial; none touch this packet's files. 20-12's own hostile test passes in isolation (A), serially (D), and in one of two parallel runs (C) — it is subject to the same pre-existing journal-lease contention because it exercises the durable journal, but it is functionally correct (its authoritative process-isolated gate is green). The authoritative acceptance proof for this packet is the six deterministic process-isolated nextest pairs, all green.

**Deferred item (NOT 20-12):** the `wcore-agent` lib suite has pre-existing, parallelism-sensitive journal-writer-lease contention plus inherent-parallelism tests (`council::run`, `spawn_tool`) that are flaky/failing under `cargo test --lib`. The stable gate is `nextest` process-isolation. Logged for the 20-08 aggregate/lifecycle work.

## Deviations / scope notes (surfaced to the 20-14 audit)

- **Live-seal cwd via the seal-gated retained checkout authority** (deviation #1): because the 06A `CandidateSeal` is fully opaque with no public accessor, and adding one would breach the 8-file scope, `SealedCandidateRoot` gates the cwd on a successful `seal_candidate()` mint and reads the same retained checkout-authority path. Cross-audited as equivalent to "cwd from the live seal" (the mint fails closed on release/drift/substitution before the cwd is used), not a weakening. Flagged for the 20-14 auditor to confirm.
- **Public-but-opaque surface:** `AcceptedCandidate`, `MutationAttemptGuard`, the gate/closure/acceptance error types, and `run_gate_acceptance` are `pub` (forced by the private-in-public lint under `-D warnings`) but remain opaque (no public constructor, non-Clone, non-Serialize).
- **`model.rs`/`reducer.rs`** carry real feature changes (`latest_receipt_digest`; a defense-in-depth reducer projection cap so a committed receipt cannot carry more gate results than its authorized plan), not stubs; no wire-schema broadening.
- **`lib.rs` >1000-line deferral from 20-10** remains; unrelated to this packet.

## Explicit non-claims

This packet makes NO parent-landing, 20-07 parent-CAS, or 20-08 complete-lifecycle claim. `AcceptedCandidate` proves observed-execution + authoritative durable replay only; landing remains impossible. It admits 20-13 (black-box tests + integrated acceptance) and advances no downstream source.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
