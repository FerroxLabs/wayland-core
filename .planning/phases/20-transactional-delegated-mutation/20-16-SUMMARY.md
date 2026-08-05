---
phase: 20-transactional-delegated-mutation
plan: "16"
subsystem: [agent, swarm, cli]
tags: [independent-review, non-author, exact-source, all-severity, asvs-level-2, full-lifecycle, anvil-landing, f20-16, repaired-successor]
requires: ["20-08"]
provides:
  - Fresh non-author all-severity exact-source review of the REPAIRED Phase 20 lifecycle construction (20-08 successor 6937ef6) plus native-UAT machinery
  - Zero-finding qualification admitting preparation plan 20-17 (does NOT admit 20-18, completes no requirement)
affects: ["20-17"]
key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-08-INDEPENDENT-REVIEW.md
  modified: []
key-decisions:
  - "This review re-runs 20-16 against the REPAIRED 20-08 successor 6937ef6 (the original 5e665ec candidate's source-only 20-16 PASS did not carry because its native UAT + full test suite later surfaced pre-existing breakage; every candidate SHA change invalidates the prior review). The 11 core 20-08 files are byte-identical 5e665ec->6937ef6 (git diff = 0), so the prior CLEAN core review carries and this review concentrates all-severity scrutiny on the repair delta + confirms the untouched core."
  - "A fresh non-author reviewer (reviewer_id wayland-f20-16-repair-review, distinct from source_executor_id wayland-f20-repair-executor) prosecuted the EXACT source SHA 6937ef6 / tree 6db6fc85 across three lenses (code review, gsd-validate-phase, gsd-secure-phase ASVS Level 2 with security_block_on=low) and recorded one schema-validated wayland-core.phase20-independent-review.v1 JSON object changing only the review path. A SECOND independent adversarial confirmer (wayland-f20-16-adversarial-confirmer) tasked to REFUTE the PASS returned CONFIRMED-CLEAN after tracing the never-compiled Windows module tree depth-by-depth, extracting bollard 0.17.1 to confirm DEFAULT_TIMEOUT=120, and proving the macOS acceptance tests can only false-negative (flaky-fail), never false-green."
  - "Zero findings at every severity: the f20-16 checks all_severity / asvs_level_2 / code_review / phase_validation are all PASS; native_macos and native_windows are the only deferred checks (external UAT at 20-18, prepared by 20-17)."
  - "First-hand receipts re-run at the exact source 6937ef6: the committed-HEAD Hetzner aggregate nextest run --workspace --profile ci = 11509 passed / 0 failed / 48 skipped (includes transactional_delegated_mutation_test 9/9 and anvil_forge_transaction 5/5); clippy -p wcore-agent -p wcore-swarm --all-targets --all-features -D warnings clean; the native-UAT-proof node test 34/34 — every gate exit 0."
  - "The repair delta (8 commits 5e665ec..6937ef6) independently judged SOUND at the mechanism level: b5613b0 Windows windows_impl module-path fix (super-chain depths exact, would compile on Windows), 61b599c docker.rs inline 120 (bollard default, behavior-preserving), b3ebf23+47a7a4c genuinely-falsifiable macOS containment tests, 3383a46+95e1b98 fail-closed proof script with --no-tests=fail on every target + a security-meaningful descoped target-5 refusal proof, 1902a64 test-only parent-workspace binding, af295e7 genuinely restores the fail-closed 'parent workspace authority is not bound' coverage 20-05 had silently defeated (production guard byte-unchanged), bb49d61 test-only F14 WSA1 journal-frame handling (product parser untouched, no recovery defect masked), 6937ef6 corpus provenance re-pin (digests-only, schema_digest + all specs/counts unchanged, adversarial poison values preserved)."
requirements-completed: []
duration: n/a
completed: 2026-07-22
status: complete
reviewer_id: wayland-f20-16-repair-review
source_executor_id: wayland-f20-repair-executor
source_sha: 6937ef61aa2ad2074dd7875f9cde2369fc104461
source_tree: 6db6fc859539b43f083aa0a22f3e3e0a014721ae
review_base_sha: af645aceb32d6c0ce835698b64dc72e9898e5296
review_sha: bc0f6d52c0db3ac21e644728e963b142fbd3b8a8
disposition: PASS
findings:
  blocker: 0
  critical: 0
  high: 0
  medium: 0
  low: 0
deferred: [native_macos, native_windows]
coverage:
  - id: SC1
    description: "A distinct non-author reviewer code-reviewed, phase-validated, and ASVS-Level-2 security-prosecuted the exact repaired 20-08 source at all severities with zero findings, independently confirmed by a second adversarial verifier."
    verification:
      - kind: other
        ref: "verify-review-result.mjs f20-16 -> review-result-ok (source 6937ef6/tree 6db6fc85, reviewer wayland-f20-16-repair-review != source_executor wayland-f20-repair-executor); second reviewer wayland-f20-16-adversarial-confirmer returned CONFIRMED-CLEAN"
        status: pass
  - id: SC2
    description: "Sole-parent, review-record-only, byte-identical-source proof over the metadata-only 20-08 successor chain."
    verification:
      - kind: other
        ref: "verify-review-pair.sh -> review-pair-ok (source 6937ef6 -> review_base af645ac -> review bc0f6d5); metadata chain 6937ef6->af645ac touches only 20-08-SUMMARY.md, summary changed exactly once; review commit changes only 20-08-INDEPENDENT-REVIEW.md; all 12 reviewed source paths byte-identical across the chain"
        status: pass
  - id: SC3
    description: "First-hand committed-HEAD Hetzner + native-node receipts belong to the exact repaired source and are green."
    verification:
      - kind: other
        ref: "Hetzner: nextest --workspace --profile ci @6937ef6 = 11509 pass / 0 fail (incl. transactional_delegated_mutation_test 9/9 + anvil_forge_transaction 5/5); clippy wcore-agent+wcore-swarm --all-targets --all-features -D warnings clean exit 0. Mac: node --test f20-native-uat-proof.test.mjs 34/34 exit 0."
        status: pass
  - id: SC4
    description: "Only a zero-finding PASS admits preparation plan 20-17; 20-18 remains blocked and no requirement is completed."
    verification:
      - kind: other
        ref: "review JSON disposition=PASS, findings all 0, checks all four PASS, deferred=[native_macos,native_windows]; requirements-completed: []"
        status: pass
---

# Phase 20 Plan 16: Independent All-Severity Exact-Source Review of the REPAIRED Final Lifecycle (20-08 successor 6937ef6) — PASS

**A fresh non-author reviewer independently prosecuted the exact repaired Phase 20 construction (20-08 successor `6937ef6`) across code review, phase validation, and ASVS Level 2 security at every severity, re-ran the first-hand committed-HEAD receipts, and recorded a zero-finding `f20-16` PASS; a second independent adversarial verifier tasked to refute the PASS returned CONFIRMED-CLEAN. The repaired construction is qualified; preparation plan 20-17 is admitted. 20-18 stays blocked; no requirement is completed.**

## Why a re-review

The original 20-08 construction `5e665ec` earned a source-only 20-16 PASS, but its Sean-authorized native UAT (run 29886894436) and — critically — its full test suite then surfaced pre-existing breakage the source-only review could not see: the wcore-sandbox Windows lib would not compile (parked `windows_impl` module-path debt), docker.rs referenced a private bollard const under `--features live-docker`, the native macOS proof script pointed at a Windows-only test with two acceptance tests unwritten, four test fixtures were stale (2 spawn_tool durable + 2 fail-closed security), the F14 sigkill journal reader predated the WSA1 snapshot-authority frame, and the Desktop-contract provenance digests were stale. The repair fixed all of it in the successor `6937ef6`. Every candidate SHA change invalidates the prior review, so 20-16 is re-run here against the exact repaired source.

## Result

- **Disposition:** PASS. Findings: 0 blocker / 0 critical / 0 high / 0 medium / 0 low.
- **Checks:** `all_severity` PASS, `asvs_level_2` PASS, `code_review` PASS, `phase_validation` PASS. Deferred: `native_macos`, `native_windows` (external UAT, prepared at 20-17, executed at 20-18).
- **Identity:** reviewer `wayland-f20-16-repair-review` ≠ source executor `wayland-f20-repair-executor`; second independent verifier `wayland-f20-16-adversarial-confirmer` CONFIRMED-CLEAN.
- **Exact source:** `6937ef6…` / tree `6db6fc85…` (recorded by 20-08-SUMMARY.md; `git rev-parse 6937ef6^{tree}` confirmed = `6db6fc85`).
- **Qualification tuple:** `(source_sha 6937ef61aa2ad2074dd7875f9cde2369fc104461, source_tree 6db6fc859539b43f083aa0a22f3e3e0a014721ae, review_base_sha af645aceb32d6c0ce835698b64dc72e9898e5296, review_sha bc0f6d52c0db3ac21e644728e963b142fbd3b8a8)`.
- **Verifiers:** `verify-review-pair.sh` → `review-pair-ok`; `verify-review-result.mjs f20-16` → `review-result-ok`; `verify-task-scope.sh` → `scope-ok paths=1`.

## Method

Two fresh non-author reviewers prosecuted the exact `6937ef6` source. The 11 core 20-08 files (`landing.rs`, `gate_authorization.rs`, `tool.rs`, `forge.rs`, `engine.rs`, `child_transaction.rs`, `gate_executor.rs`, `parent.rs`, `spawner.rs`, `worktree_manager.rs`, `worktree.rs`) are byte-identical `5e665ec → 6937ef6` (`git diff` = 0), so the prior CLEAN core review carries; all-severity scrutiny concentrated on the 8-commit repair delta plus a re-confirmation that the delta touches nothing security-load-bearing (only 4 files changed source-wise: `spawn_tool.rs` #[cfg(test)] only, `docker.rs` const inline, and the two cfg(windows) `windows_impl` files dead on Linux/Mac). First-hand receipts were re-run at the exact committed HEAD; native macOS/Windows execution was classified DEFERRED to 20-18, not passed from source.

## All-severity findings

| Severity | Count |
|----------|-------|
| Blocker  | 0 |
| Critical | 0 |
| Medium   | 0 |
| High     | 0 |
| Low      | 0 |

## Repair delta — independently judged SOUND (both reviewers)

1. **b5613b0 windows_impl module paths** — cfg(windows)-only; super-chain depths exact (`reserve_output`/`BUFFERED_OUTPUT_LIMIT_BYTES` pub(crate) in backends → `super::super::super::`; `probe_single_flight` private in appcontainer → `super::super::`; `SharedJob::new` pub(super) replaces sibling private-field init). Would compile on Windows; no off-by-one; paths/visibility only, zero runtime behavior change.
2. **61b599c docker timeout** — bollard 0.17.1 `DEFAULT_TIMEOUT: u64 = 120` (seconds); inline `120` is behavior-preserving.
3–4. **b3ebf23 + 47a7a4c macOS acceptance tests** — genuinely falsifiable: retained-directory negative path asserts both non-zero exit AND the escapee file never exists; process-tree containment blocks on the leaked descendant's stdout pipe to the 30s manifest timeout with a 20s bound below the 45s sleep, so timing can only false-negative, never false-green. Correctly gated (`#![cfg(target_os="macos")]` + `#[ignore]` + `WAYLAND_SANDBOX_LIVE_MACOS` + `is_available()`).
5. **3383a46 + 95e1b98 proof script** — fails closed (repo-root, clean tree, exact commit AND tree, acceptance env, Darwin, live `docker info`); `--no-tests=fail` on the single shared `run_target` used by all 8 targets under `set -euo pipefail`; descoped target-5 hits the real `sandbox_exec_is_refused_before_descendant_escape_can_spawn` asserting the security-critical macOS refusal of the non-hard-containment backend.
6. **1902a64 spawn_tool** — both hunks inside `#[cfg(test)]`; no production code.
7. **af295e7 fail-closed restore** — `bind_durable_session_only` leaves parent_workspace genuinely unbound; both repointed tests assert the specific "parent workspace authority is not bound" diagnostic (+ turns==0); production guard byte-unchanged; restores coverage 20-05's always-bind-parent change had silently defeated.
8. **bb49d61 F14 WSA1** — test-only; product `session_journal.rs` untouched; helper accepts both magics at the identical 12-byte-header + body + 32-byte-digest stride and walks past WSA1 bindings while decoding WJ01, masking no recovery defect.
9. **6937ef6 corpus** — digests-only (`fixture_digest` + `source_inputs_digest` re-pinned; `schema_digest` and all specs/capabilities/counts/child_types byte-identical; adversarial poison values preserved). Wire contract unchanged.

## Evidence (first-hand, exact source 6937ef6)

- Hetzner aggregate: `nextest run --workspace --profile ci --no-fail-fast` → 11509 passed / 0 failed / 48 skipped (incl. `transactional_delegated_mutation_test` 9/9, `anvil_forge_transaction` 5/5 with `production_landing::drive_climb_full_lands_the_winner_surface_for_accept`).
- Hetzner clippy: `-p wcore-agent -p wcore-swarm --all-targets --all-features -- -D warnings` → clean, exit 0.
- Mac: `node --test scripts/f20-native-uat-proof.test.mjs` → 34/34, exit 0.
- Byte-identity: `git diff --quiet 5e665ec 6937ef6 -- <11 core files>` → identical.

## Admission

A zero-finding PASS in the exact repaired review tuple **admits preparation plan 20-17**. It does **NOT** admit 20-18 (external native macOS/Windows UAT remains blocked until 20-17 durably summarizes the matching pending tuple), and it completes **no** Phase 20 requirement, makes no native/aggregate/packaging/release/phase-completion claim.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-22*
