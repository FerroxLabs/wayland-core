---
phase: 20-transactional-delegated-mutation
plan: "16"
subsystem: [agent, swarm, cli]
tags: [independent-review, non-author, exact-source, all-severity, asvs-level-2, full-lifecycle, anvil-landing, f20-16]
requires: ["20-08"]
provides:
  - Fresh non-author all-severity exact-source review of the final Phase 20 lifecycle construction (20-08) plus native-UAT machinery
  - Zero-finding qualification admitting preparation plan 20-17 (does NOT admit 20-18, completes no requirement)
affects: ["20-17"]
key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-08-INDEPENDENT-REVIEW.md
  modified: []
key-decisions:
  - "A fresh non-author reviewer (reviewer_id wayland-f20-16-independent-review, distinct from source_executor_id wayland-f20-08-builder) prosecuted the EXACT 20-08 source SHA 5e665ec / tree e5dcf77 across three lenses (code review, gsd-validate-phase, gsd-secure-phase ASVS Level 2 with security_block_on=low) and recorded one schema-validated wayland-core.phase20-independent-review.v1 JSON object changing only the review path."
  - "Zero findings at every severity: the f20-16 checks all_severity / asvs_level_2 / code_review / phase_validation are all PASS; native_macos and native_windows are the only deferred checks (external UAT at 20-18, prepared by 20-17)."
  - "First-hand focused proof re-run at the exact source on the committed-HEAD Hetzner harness: transactional_delegated_mutation_test 9/9, anvil_forge_transaction 5/5, clippy -p wcore-agent -p wcore-swarm --all-targets --all-features -D warnings clean, and the native-UAT-proof node test 34/34 — every gate exit 0."
  - "All three documented 20-08 open_items independently re-judged as genuine non-flaws against the source bytes: (1) the builder-write→seal content-capture caveat is a TEST-harness wiring limitation — production content-capture is proven at the lower seam (land_selected_winner_drives_production_chain_to_landed); (2) the mem::forget retention is genuinely memory-safe (TransactionWorkspace has no Drop; leaking its Arc<TransactionCleanup>/Arc<authorities> is safe, no UAF) and is a bounded, documented Desktop-owned-GC interim; (3) input_identities sealing paths-not-content does not weaken containment (enforced by the read-only-candidate + scratch-only-write + net-deny + empty-env manifest, not by input_identities) and is_valid_gate_id is re-validated fail-closed by canonical_digest."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
reviewer_id: wayland-f20-16-independent-review
source_executor_id: wayland-f20-08-builder
source_sha: 5e665ec5911fa2a118de70b498b8f0e2841d50ba
source_tree: e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba
review_base_sha: 52d0495dd74bbd8316e805c7a9d5f1fb564c948d
review_sha: 33c7938d2f47c165e73410c99733445d5772260c
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
    description: "A distinct non-author reviewer code-reviewed, phase-validated, and ASVS-Level-2 security-prosecuted the exact 20-08 source at all severities with zero findings."
    verification:
      - kind: other
        ref: "verify-review-result.mjs f20-16 → review-result-ok (source 5e665ec/tree e5dcf77, reviewer wayland-f20-16-independent-review ≠ source_executor wayland-f20-08-builder)"
        status: pass
  - id: SC2
    description: "Sole-parent, review-record-only, byte-identical-source proof over the metadata-only 20-08 chain."
    verification:
      - kind: other
        ref: "verify-review-pair.sh → review-pair-ok (source 5e665ec → review_base 52d0495 → review 33c7938); metadata chain 5e665ec→52d0495 touches only STATE.md + 20-08-SUMMARY.md, summary changed exactly once"
        status: pass
  - id: SC3
    description: "First-hand focused committed-HEAD Hetzner + native-node receipts belong to the exact source and are green."
    verification:
      - kind: other
        ref: "Hetzner: f20-16-e2e transactional_delegated_mutation_test 9/9 exit 0; f20-16-forge anvil_forge_transaction 5/5 exit 0; f20-16-clippy wcore-agent+wcore-swarm -D warnings exit 0. Mac: node --test f20-native-uat-proof.test.mjs 34/34 exit 0."
        status: pass
  - id: SC4
    description: "Only a zero-finding PASS admits preparation plan 20-17; 20-18 remains blocked and no requirement is completed."
    verification:
      - kind: other
        ref: "review JSON disposition=PASS, findings all 0, checks all four PASS, deferred=[native_macos,native_windows]; requirements-completed: []"
        status: pass
---

# Phase 20 Plan 16: Independent All-Severity Exact-Source Review of the Final Lifecycle (20-08) — PASS

**A fresh non-author reviewer independently prosecuted the exact final Phase 20 construction (20-08 source `5e665ec`) across code review, phase validation, and ASVS Level 2 security at every severity, re-ran the focused committed-HEAD receipts first-hand, and recorded a zero-finding `f20-16` PASS. The construction is qualified; preparation plan 20-17 is admitted. 20-18 stays blocked; no requirement is completed.**

## Result

- **Disposition:** PASS. Findings: 0 blocker / 0 critical / 0 high / 0 medium / 0 low.
- **Checks:** `all_severity` PASS, `asvs_level_2` PASS, `code_review` PASS, `phase_validation` PASS. Deferred: `native_macos`, `native_windows` (external UAT, prepared at 20-17, executed at 20-18).
- **Identity:** reviewer `wayland-f20-16-independent-review` ≠ source executor `wayland-f20-08-builder`.
- **Exact source:** `5e665ec…` / tree `e5dcf77…` (recorded by 20-08-SUMMARY.md; `git rev-parse 5e665ec^{tree}` confirmed = `e5dcf77`).
- **Qualification tuple:** `(source_sha 5e665ec5911fa2a118de70b498b8f0e2841d50ba, source_tree e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba, review_base_sha 52d0495dd74bbd8316e805c7a9d5f1fb564c948d, review_sha 33c7938d2f47c165e73410c99733445d5772260c)`.
- **Verifiers:** `verify-review-pair.sh` → `review-pair-ok`; `verify-review-result.mjs f20-16` → `review-result-ok`.

## Method

Reviewed the exact 20-08 source bytes (the metadata chain `5e665ec → 52d0495` changes only `.planning/STATE.md` and `20-08-SUMMARY.md`, so every reviewed source blob is byte-identical at source and review base — independently confirmed per-path). Three adversarial lenses were applied to the terminal-landing composition (`landing.rs`, `gate_authorization.rs`, `tool.rs`, `forge.rs` `drive_climb_full`/`attempt_landing`, `engine.rs` seams), the durable lifecycle (`child_transaction.rs`, `child_transaction/gate_executor.rs`, `child_transaction/parent.rs`, `spawner/mutation_workspace.rs`, `worktree_manager.rs`, `worktree.rs`), the hostile E2E tests, and the native-UAT machinery (`f20-native-uat-proof.mjs`/`.test.mjs`, `f20-native-macos-proof.sh`, `f20-native-windows-proof.ps1`, `nightly-windows-soak.yml`, the AGENTS.md narrow-publication rule). First-hand focused receipts were re-run at the exact committed HEAD on the Hetzner harness; native macOS/Windows execution was classified DEFERRED to 20-18, not passed from source.

## All-severity findings

| Severity | Count |
|----------|-------|
| Blocker  | 0 |
| Critical | 0 |
| High     | 0 |
| Medium   | 0 |
| Low      | 0 |

## What the review verified at the source

- **Effects ordering:** `emit_receipt` runs strictly BEFORE landing consumes the winner (`forge.rs`), so the receipt digests the still-live winner checkout that landing then terminalizes.
- **Candidate selection / one-dispatch:** exactly one selected winner (or none) reaches `land_selected_winner`; every loser is RAII-terminalized before parent preparation and has no reference path to its own landing authority. `into_landing_authority` fail-closes to `None` on a released/drifted/substituted checkout.
- **Forge terminal reporting:** the tool renders only the authoritative parent landing outcome (Landed/Conflict/Incomplete/RolledBack/RecoveryRequired/Failed/None); a landing failure is reported into `ClimbOutcome.landing`, never crashing the climb; no manual merge/cherry-pick escape text exists (landing is CAS-or-fail with no 3-way-merge codepath).
- **Retained-snapshot authority & parent CAS (20-07):** the opaque `ChildTransactionAuthority` (no serde, no public constructor) is bound and revalidated through open → accept → land → rollback → cleanup; landing advances `target_ref` via `update-ref <ref> <new> <old=base>` under a parent lock — a racing parent edit yields an explicit Conflict, never a silent overwrite.
- **Rollback / restart convergence:** durable append ordering (LandingPrepared before CAS; RefAdvanced→Projected→Landed only after coherent projection) plus an identity-matched recovery matrix converge to exactly old-or-new coherent state; rollback/cleanup are idempotent, identity-bound, and refuse foreign state (descendant-loan refusal).
- **06C hard-containment re-run:** `gate_authorization` pins the 06C `AuthorizedGateClosure` digest (domain `wayland-core:authorized-gate-closure:v1`), NOT the Anvil `anvil-gate-closure-v1` digest; empty-env parity holds (sealed empty env == the `HardContainmentFilesystem::to_manifest` empty env); the candidate is read-only, only the winner's own scratch is writable, network is denied (`bwrap --unshare-all --clearenv`).
- **User repository never opened/mutated:** the integration checkout is a standalone `git clone --no-local --no-hardlinks --single-branch` at the exact user tip, with post-clone verification (origin removed, exact head, target branch, clean tree, in-tree `.git`, linked-worktree refused, and an explicit failure if `objects/info/alternates` exists); all landing git runs `-C <clone>`. The user's real repository is only read to clone from, never mutated.
- **Memory safety:** no `unsafe impl Send`/`unsafe impl Sync` anywhere in the lifecycle (`LiveCandidateRoot: Send + Sync` is auto-derived and sound); the `mem::forget(integ)` retention is memory-safe (no Drop on `TransactionWorkspace`; leaking Arcs is safe, no UAF).
- **Native machinery fail-closed before external mutation:** the UAT proof helper is verification-only (no-follow fd reads, exact-byte retention, exclusive log creation, exactly-once/in-order markers, one-dispatch `bindRun`, idempotent request/authorization); the shell/PowerShell helpers gate on `WAYLAND_F20_NATIVE_ACCEPTANCE=1` + repo-root + clean + exact commit AND tree; candidate CI mode grants only `contents: read`, no secrets, no persisted cache, ephemeral macOS runner deregister. AGENTS.md scopes the sole `just push` exception to that one helper's UAT ref only.

## The three documented 20-08 open_items — independently re-judged as non-flaws

1. **Builder-write→seal content-capture caveat (test-harness):** the `MockLlmProvider` builder's Write does not reach the sealed checkout in the forge-boundary test, so that one test's landed tree equals base. This is a harness wiring limitation, not a production landing bug — the production seal→synthesize→CAS content capture is proven at the lower seam by `land_selected_winner_drives_production_chain_to_landed` (stages a real file, asserts it present in the integration checkout after landing, then rollback removes it).
2. **mem::forget persist() follow-up:** genuinely memory-safe and bounded; the retained-on-disk clone is Sean's explicit surface-for-accept + Desktop-owned-GC decision. Observation: the FOLLOW-UP `TransactionWorkspace::persist()` should tear down the lease thread + flock + reservation-registry entry, not only "byte accounting" — noted for 20-17/successor scoping; not a defect in the delivered behavior.
3. **2C OBS (input_identities paths-not-content; looser is_valid_gate_id):** input_identities only needs a deterministic anti-substitution identity; containment is enforced by the read-only candidate + scratch-only write + net-deny + empty-env manifest, and the candidate cannot change under the read-only re-run. `is_valid_gate_id` is re-validated fail-closed by `canonical_digest`. Both are faithful to 06C, not weaknesses.

## Evidence (first-hand, exact source)

- Hetzner `f20-16-e2e`: `test -p wcore-agent --test transactional_delegated_mutation_test` → 9/9, exit 0.
- Hetzner `f20-16-forge`: `test -p wcore-agent --test anvil_forge_transaction` → 5/5, exit 0.
- Hetzner `f20-16-clippy`: `clippy -p wcore-agent -p wcore-swarm --all-targets --all-features -- -D warnings` → clean, exit 0.
- Mac: `node --test scripts/f20-native-uat-proof.test.mjs` → 34/34, exit 0.

## Admission

A zero-finding PASS in the exact review tuple **admits preparation plan 20-17**. It does **NOT** admit 20-18 (external native macOS/Windows UAT remains blocked until 20-17 durably summarizes the matching pending tuple), and it completes **no** Phase 20 requirement, makes no native/aggregate/packaging/release/phase-completion claim.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
