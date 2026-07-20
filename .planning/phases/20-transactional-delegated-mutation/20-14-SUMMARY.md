---
phase: 20-transactional-delegated-mutation
plan: "14"
subsystem: [swarm, sandbox, agent]
tags: [independent-audit, non-author, exact-candidate, all-severity, 06a-06d, f20-14]
requires: ["20-13"]
provides:
  - Fresh non-author all-severity independent audit of the exact integrated 06A-06D delegated-mutation candidate
  - Zero-finding qualification admitting 20-05 and 20-07
affects: [20-05, 20-07]
key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-14-INDEPENDENT-AUDIT.md
  modified: []
key-decisions:
  - "A fresh non-author auditor (reviewer_id wayland-f20-14-independent-audit, distinct from every source author wayland-f20-06/10/12/13-builder; source_executor_id wayland-f20-13-builder) prosecuted the EXACT integrated 06A-06D candidate (06D source ace4bd2 / tree 25c2c6c8) at all severities and recorded one schema-validated wayland-core.phase20-independent-review.v1 JSON object changing only the audit path."
  - "Zero findings at every severity: the f20-14 checks all_severity / evidence_integrity / integration_authority are all PASS; native_macos and native_windows are the only deferred checks (validated at 20-17/20-18 UAT)."
  - "The audit independently certified the load-bearing acceptance-authority properties: crate-private mint unreachability (AcceptanceMachine::accept pub(crate), AcceptedCandidate/HardContainmentAuthority/CandidateSeal mint private), opaque !Clone/!serde authorities, observed results produced ONLY from consumed live-containment spawns, the SealedCandidateRoot cwd-from-seal derivation (fresh seal_candidate() re-proof before the retained checkout path is used), the AcceptedCandidate seal-before-guard drop order (no outstanding-loan on cleanup), and the one-use verify_no_drift re-checking all 7 bound fields."
  - "A prior 20-14 run correctly FAILed at the admission tree-consistency gate on a mis-recorded 20-13 source_tree (a builder reported an orphan intermediate commit's tree); that was corrected metadata-only (20-13-SUMMARY source_tree → 25c2c6c8, 13-metadata amended 19f2e16 → 9d67fdd) with NO change to the 06D source, and this audit passed both verifiers on the corrected consistent candidate."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
reviewer_id: wayland-f20-14-independent-audit
source_executor_id: wayland-f20-13-builder
source_sha: ace4bd26fa3d831b2129ce319248652dbc25f5b7
source_tree: 25c2c6c8b5d5d6eed7c33fc8e89c1c98619e2c5d
review_base_sha: 9d67fdd90a47486b35671da73f4264ecc96dfdaa
review_sha: bc907b73df45fd3ffcdf2c7b2c1363d5405e2440
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
    description: "A distinct non-author auditor reviewed the exact 06A-06D candidate at all severities with zero findings."
    verification:
      - kind: other
        ref: "verify-review-result.mjs f20-14 → review-result-ok (source ace4bd2/tree 25c2c6c8, reviewer wayland-f20-14-independent-audit ≠ every source author)"
        status: pass
  - id: SC2
    description: "Sole-parent, audit-record-only, byte-identical-source proof over the metadata-only 06D chain."
    verification:
      - kind: other
        ref: "verify-review-pair.sh → review-pair-ok (source ace4bd2 → review_base 9d67fdd → review bc907b7); both upstream tuples f20-09/f20-11 re-proven green"
        status: pass
  - id: SC3
    description: "Only a zero-finding PASS admits 20-05 and 20-07."
    verification:
      - kind: other
        ref: "audit JSON disposition=PASS, findings all 0, checks all_severity/evidence_integrity/integration_authority=PASS, deferred=[native_macos,native_windows]"
        status: pass
---

# Phase 20 Plan 14: Independent All-Severity Audit of the 06A-06D Candidate — PASS

**A fresh non-author auditor independently prosecuted the exact integrated 06A-06D delegated-mutation candidate (06D source `ace4bd2`) at every severity and recorded a zero-finding `f20-14` PASS. The integrated candidate is qualified; 20-05 and 20-07 are admitted.**

## Result

- **Disposition:** PASS. Findings: 0 blocker / 0 critical / 0 high / 0 medium / 0 low.
- **Checks:** `all_severity` PASS, `evidence_integrity` PASS, `integration_authority` PASS. Deferred: `native_macos`, `native_windows`.
- **Identity:** reviewer `wayland-f20-14-independent-audit` ≠ every source author (`wayland-f20-06/10/12/13-builder`); source executor `wayland-f20-13-builder`.
- **Exact candidate:** 06D source `ace4bd2…` / tree `25c2c6c8…`.
- **Qualification tuple:** `(source_sha ace4bd2, source_tree 25c2c6c8, review_base_sha 9d67fdd, review_sha bc907b7)`.
- **Verifiers (independently re-run by the orchestrator):** `verify-review-pair.sh` → `review-pair-ok`; `verify-review-result.mjs f20-14` → `review-result-ok`.

## What the audit certified (all independently, at the source)

- **Acceptance-authority forgery resistance:** `AcceptanceMachine::accept` is `pub(crate)`, `AcceptedCandidate::mint` / `HardContainmentAuthority::mint` / `CandidateSeal::mint` are crate-private, and observed results are module-private and produced ONLY from a consumed live-containment spawn — no model claim, candidate claim, scripted runner, serialized receipt, or injected status can mint or retain acceptance; non-qualifying backends keep the `None`/`PolicyNotSupported` trait defaults and are structurally incapable of minting.
- **SealedCandidateRoot cwd-from-seal — sound:** `resolve_root` mints a fresh `CandidateSeal` (re-proving execution authority + revalidating the source manifest, fail-closed on release/drift/identity-change/`.git`-poisoning/substitution) BEFORE returning the retained `checkout_authority().display_path()`; path and seal both derive from the single retained workspace handle, so the cwd can only be the exact live, clean, sealed checkout, and it is re-bound at spawn via the authority's `spawn_identity`.
- **AcceptedCandidate drop order — sound:** field order `seal` before `guard` releases the seal's cloned checkout authority + cleanup-liveness Arc before the guard terminalizes, so guard cleanup never observes an outstanding seal loan (`TransactionCleanup::release` refuses while `has_outstanding_loans()`).
- **One-use `verify_no_drift` — sound:** re-checks all 7 bound fields and consumes `self` by value.
- **windows_impl regression — NOT in any audited 06A-06D blob, correctly deferred:** the only `windows_impl` reference in the audited set is the `#[cfg(windows)] mod windows_impl {…}` wrapper in `appcontainer.rs`; the broken Win32 FFI (20-02 `4dcd62a`) lives in non-audited `appcontainer/{command,handles,process}.rs`, routed to 20-08 with a validated patch and deferred to native-Windows UAT.
- **Gate-less positive-plan bound (20-13):** acceptable, not a gap — a black-box caller cannot author a non-empty durable plan because closure digests are crate-private (a real security property); gate-execution stages are covered by in-crate unit tests and the live falsifiable descendant-reaping black-box test.

## Audit conduct

The audit was fresh and adversarial. A prior run correctly fail-closed at the admission tree-consistency gate on a mis-recorded 20-13 `source_tree` (a builder reported an orphaned intermediate commit's tree); the orchestrator corrected it metadata-only (recorded tree → the true `25c2c6c8`, 13-metadata amended, 06D source unchanged) and re-ran. This audit passed both verifiers on the corrected, internally-consistent candidate with zero findings — the evidence-integrity gate demonstrably works.

## Non-claims

This audit qualifies the integrated 06A-06D construction only; it makes no parent-landing, full-lifecycle, native-runtime, release, or deployment claim. It admits 20-05 and 20-07.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
