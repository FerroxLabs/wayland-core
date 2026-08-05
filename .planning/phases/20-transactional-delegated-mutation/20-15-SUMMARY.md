---
phase: 20-transactional-delegated-mutation
plan: "15"
subsystem: review
tags: [independent-review, non-author, exact-source, all-severity, delegated-mutation]
requires: ["20-03"]
provides:
  - Fresh non-author all-severity independent review of the exact 20-03 isolation-substrate source
  - Zero-finding PASS qualification tuple admitting plan 20-04
affects: [20-04]
key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-03-INDEPENDENT-REVIEW.md
  modified: []
key-decisions:
  - "Reviewed the exact clean product source d343fc72 (tree 7eca6e8) — the 20-03 successor that incorporates the review-driven fail-closed-cleanup repair — not the pre-repair candidate."
  - "The review is recorded as one schema-validated JSON object (wayland-core.phase20-independent-review.v1) with distinct source-executor and reviewer identities; only the zero-finding PASS admits 20-04."
requirements-completed: []
duration: n/a
completed: 2026-07-20
status: complete
review_tuple:
  source_sha: d343fc720c38c05d0097821ff3117f88e12fa203
  source_tree: 7eca6e83107f1d7d1692f32ae17d6ed6ddf92135
  review_base_sha: 13923f63376f69a241b6c2b7e15c0508f4da03d9
  review_sha: c2e4d5948f2e30a46e1d85a0a60c120496913931
---

# Phase 20 Plan 15: Independent Review of the 20-03 Isolation Substrate

**A fresh, non-author, all-severity review of the exact 20-03 product source `d343fc72` returned zero findings across every severity, admitting plan 20-04.**

## Review Tuple

- **source_sha:** `d343fc72` — tree `7eca6e8` (the 20-03 successor including the review-driven fail-closed-cleanup repair)
- **review_base_sha:** `13923f6` (the 20-03 SUMMARY/STATE metadata commit; sole-parent, merge-free, summary changed exactly once)
- **review_sha:** `c2e4d5948f2e30a46e1d85a0a60c120496913931` (sole-parent child of the review base; changes only `20-03-INDEPENDENT-REVIEW.md`)
- **source_executor_id:** `wayland-f20-03-builder`
- **reviewer_id:** `wayland-f20-15-independent-review` (distinct from the source executor)

## Method

The exact 41-path 20-03 source was reviewed adversarially across three concern lenses — retained filesystem authority + sandbox backends, public Swarm dispatch lifecycle, and Bash containment — plus a focused re-review of the repair, without modifying any source blob. Every declared must-have was reconciled against the actual source bytes and the committed-head Linux receipts; type presence, source presence, and builder self-checks alone were treated as insufficient.

## Findings (all severities)

| Severity | Count |
|----------|-------|
| Blocker  | 0 |
| Critical | 0 |
| High     | 0 |
| Medium   | 0 |
| Low      | 0 |

The first review pass over the pre-repair candidate `bcd8463` surfaced one HIGH (detached `Drop` cleanup could delete the checkout and release the reservation while a worker descendant still held the retained checkout descriptor, defeating the quarantine guard) and one LOW (the Unix cleanup primitive lacked the loan self-guard the Windows path has — the same root cause). Both were repaired in the 20-03 successor `d343fc72` (fail-closed `release()` on outstanding checkout loan, plus a hostile test), the full committed-head Linux proof was re-run green, and this review of the repaired exact source records zero residual findings.

## Observation (non-blocking, not a finding)

The fix closes the exploitable defect at the `TransactionCleanup::release()` chokepoint (which reads the shared checkout loan counter before any mutation). The Unix `remove_open_dir_all`/`remove_descendants` primitives were **not** given the Windows-style self-guard; the primitive-level asymmetry with the Windows path persists. This is recorded as an observation rather than a finding because the re-review's caller analysis proved **no reachable path** deletes with a genuinely outstanding checkout loan: `release()`/`release_transaction` are now guarded, `cleanup_all` reclamation is gated by the root fd-lease (`transaction_is_active`), and the archive-transport primitive is unrelated to the swarm checkout loan. Adding a guard for a proven-unreachable failure would contradict the codebase principle "no error handling for impossible scenarios" and risk destabilizing a green, lease-timing-sensitive cleanup path. Filed as a future-hardening item (Windows/Unix primitive parity) for a later consumer that might call the Unix primitive directly.

## Checks

- **all_severity:** PASS
- **public_lifecycle:** PASS
- **retained_authority:** PASS
- **deferred:** none (native Windows/macOS execution is a 20-08 concern, not a 20-15 check)

## Evidence

All committed-head Hetzner gates were re-run green at the exact source `d343fc72`: focused sandbox/swarm/bash suites, the eight named `required_live` receipts, strict `--all-targets --all-features -- -D warnings` clippy, `fmt --all -- --check`, and the `verify-f20-03-scope.sh final` gate (`paths=41`). Topology and record integrity are enforced by `verify-review-pair.sh` and `verify-review-result.mjs` (profile `f20-15`).

## Admission

Zero findings at every severity in the exact review tuple — **plan 20-04 is admitted.** This review does not mark F20-01 or F20-02 complete and does not substitute for native Windows/macOS UAT (deferred to plan 20-08).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
