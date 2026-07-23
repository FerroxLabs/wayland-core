---
phase: 20-transactional-delegated-mutation
plan: "17"
type: execute
status: complete
completed: 2026-07-22
requirements_completed: []
# Reviewed candidate (reauthenticated from 20-16 before any request existed)
source_executor_id: wayland-f20-repair-executor
reviewer_id: wayland-f20-16-repair-review
source_sha: 6937ef61aa2ad2074dd7875f9cde2369fc104461
source_tree: 6db6fc859539b43f083aa0a22f3e3e0a014721ae
review_base_sha: af645aceb32d6c0ce835698b64dc72e9898e5296
review_sha: bc0f6d52c0db3ac21e644728e963b142fbd3b8a8
# Immutable pending native-proof tuple handed to plan 20-18
pending_request:
  kind: request
  status: pending
  candidate: 6937ef61aa2ad2074dd7875f9cde2369fc104461
  commit: 6937ef61aa2ad2074dd7875f9cde2369fc104461
  tree: 6db6fc859539b43f083aa0a22f3e3e0a014721ae
  ref: refs/f20-native-uat/6937ef61aa2ad2074dd7875f9cde2369fc104461
  runner_label: f20-macos-ephemeral-e28aca10
  image_label: f20-image-c88ccc10c6e95edb3a009f2ba55b83147d918908b5b86bf40ec72087320827c6
  nonce: 21b9a7bdda9ad365fbec84b1ef611490
request_digest: cb6f06bde019ee32814f20ff67359291d548c497efc3dbab9f7754dea986e7f1
locator_digest: c5d9ef9ca0f78b0f507c017c9fdb7a7bd616f025652595150a078e9d5ebeead9
proof_checkout: .git/f20-native-uat/6937ef61aa2ad2074dd7875f9cde2369fc104461/checkout
runner_id: 24
---

# Phase 20 Plan 17: Native-Proof Request Preparation

**One durable, exact-tuple-idempotent pending native-proof request is persisted for the exact independently-reviewed Phase 20 candidate (`6937ef6`). Every safe read-only / Git-private preparation step is complete; nothing is authorized, published, or dispatched. Admits plan 20-18.**

## What was prepared

1. **Reauthenticated the exact 20-16 qualification** before any request existed: `verify-review-pair.sh` (source `6937ef6` → review_base `af645ac` → review `bc0f6d5`, linear metadata-only, source blobs byte-identical) and `verify-review-result.mjs` profile `f20-16` (schema-valid, PASS, zero findings, evidence/focused-receipts authenticated) both returned their `-ok` lines. The reviewed source SHA equals the candidate. Focused receipts are not a separate store — the retained `f20-08`/`f20-16` gate receipts are the `evidence[]` entries inside the qualified review record, re-authenticated by `verify-review-result.mjs`.
2. **Runner preflight (read-only)** admitted **exactly one** online, idle self-hosted runner (id 24, `f20-macos-ephemeral-e28aca10`) carrying `f20-native-macos` + `f20-ephemeral` + `f20-no-ambient-secrets` + exactly one `f20-image-<64hex>` label. Zero or multiple qualifying runners would fail closed.
3. **Deterministic source-keyed read-only proof checkout** materialized at the exact reviewed source under the primary Git directory (`.git/f20-native-uat/<sha>/checkout`, `git worktree --detach 6937ef6`): verified HEAD = `6937ef61aa2ad2074dd7875f9cde2369fc104461` and tree = `6db6fc859539b43f083aa0a22f3e3e0a014721ae`, then locked read-only. The main working tree stays clean.
4. **Git-private locator + pending request** persisted as mode-0600, no-follow, single-line-JSON+LF objects under the primary Git directory (`.git/f20-native-uat/<sha>/{locator,request}`). The request (`kind:request`, `status:pending`) binds the candidate commit/tree/ref + runner name + `f20-image` label + a fresh nonce, and is validated by the helper's own `validateRequest`; re-reading through the no-follow reader round-trips clean.

## Immutable pending tuple (for 20-18)

- candidate/commit `6937ef61aa2ad2074dd7875f9cde2369fc104461`, tree `6db6fc859539b43f083aa0a22f3e3e0a014721ae`
- ref `refs/f20-native-uat/6937ef61aa2ad2074dd7875f9cde2369fc104461`
- runner `f20-macos-ephemeral-e28aca10` (id 24)
- image label `f20-image-c88ccc10c6e95edb3a009f2ba55b83147d918908b5b86bf40ec72087320827c6`
- nonce `21b9a7bdda9ad365fbec84b1ef611490`
- request digest `cb6f06bd…`, locator digest `c5d9ef9c…`

## Idempotency / crash-safety

Re-execution reopens the primary-Git-dir locator, reuses the identical pending request (same nonce), and does not repeat the focused proof or create a second request or checkout. A different tuple, malformed object, non-pending state, symlink/FIFO/directory at an authority path, or ambiguous runner fails closed. Proven: a second driver run reported `locator: reused (identical)` / `request: reused existing pending (idempotent)` with the same nonce.

## Explicit non-claims

- This plan does **not** authorize, publish, dispatch, push, retry, merge, release, tag, close an issue, delete a ref, or complete any requirement.
- Native macOS and native Windows UAT remain **pending for plan 20-18** — the single Sean-authorized dual-platform dispatch (hosted `windows-2022` + the pinned ephemeral macOS runner) against the exact candidate. Aggregate proof, requirement completion, and phase completion remain for 20-18.

## Note (tooling reconciliation, non-flaw)

`scripts/f20-native-uat-proof.mjs` is a verifier-only library (no `request` writer). Persistence therefore used a driver that imports the helper's own `validateRequest`/`reconcileRequest`/`readExactBytesNoFollow`, so the persisted bytes are exactly what the 20-18 verifier reads. No tracked source was modified (`files_modified: []` honored — only Git-private authority objects + this summary).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-22*
