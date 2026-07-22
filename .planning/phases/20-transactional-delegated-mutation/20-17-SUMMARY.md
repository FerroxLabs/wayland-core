---
phase: 20-transactional-delegated-mutation
plan: "17"
type: execute
status: complete
completed: 2026-07-22
requirements_completed: []
# Reviewed candidate (reauthenticated from 20-16 before any request existed)
source_executor_id: wayland-f20-08-builder
reviewer_id: wayland-f20-16-independent-review
source_sha: 5e665ec5911fa2a118de70b498b8f0e2841d50ba
source_tree: e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba
review_base_sha: 52d0495dd74bbd8316e805c7a9d5f1fb564c948d
review_sha: 33c7938d2f47c165e73410c99733445d5772260c
# Immutable pending native-proof tuple handed to plan 20-18
pending_request:
  kind: request
  status: pending
  candidate: 5e665ec5911fa2a118de70b498b8f0e2841d50ba
  commit: 5e665ec5911fa2a118de70b498b8f0e2841d50ba
  tree: e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba
  ref: refs/f20-native-uat/5e665ec5911fa2a118de70b498b8f0e2841d50ba
  runner_label: f20-macos-ephemeral-8292012c-e803-4c34-b533-fd83e5fc0525
  image_label: f20-image-b10cf631db3207cb2f3402f71600aeea406e94caa8f041bcc9cce8f4545d6c0f
  nonce: c3b81287aec540c86ca4a632d371324c
request_digest: 6ed9642cc867a3dff71f7fcb37073303636f8a675797a3a4f68c3b5f24e3adb3
locator_digest: 8ecc2fa6cb5e84c8c6603a710005e372abf1ff1a1974a922f1cade67ced5024b
proof_checkout: .git/f20-native-uat/5e665ec5911fa2a118de70b498b8f0e2841d50ba/checkout
runner_id: 23
---

# Phase 20 Plan 17: Native-Proof Request Preparation

**One durable, exact-tuple-idempotent pending native-proof request is persisted for the exact independently-reviewed Phase 20 candidate (`5e665ec`). Every safe read-only / Git-private preparation step is complete; nothing is authorized, published, or dispatched. Admits plan 20-18.**

## What was prepared

1. **Reauthenticated the exact 20-16 qualification** before any request existed: `verify-review-pair.sh` (source `5e665ec` → review_base `52d0495` → review `33c7938`, linear metadata-only, source blobs byte-identical) and `verify-review-result.mjs` profile `f20-16` (schema-valid, PASS, zero findings, evidence/focused-receipts authenticated) both returned their `-ok` lines. The reviewed source SHA equals the candidate. Focused receipts are not a separate store — the retained `f20-08`/`f20-16` gate receipts are the `evidence[]` entries inside the qualified review record, re-authenticated by `verify-review-result.mjs`.
2. **Runner preflight (read-only)** admitted **exactly one** online, idle self-hosted runner (id 23, `f20-macos-ephemeral-8292012c-...`) carrying `f20-native-macos` + `f20-ephemeral` + `f20-no-ambient-secrets` + exactly one `f20-image-<64hex>` label. Zero or multiple qualifying runners would fail closed.
3. **Deterministic source-keyed read-only proof checkout** materialized at the exact reviewed source under the primary Git directory (`.git/f20-native-uat/<sha>/checkout`, `git worktree --detach 5e665ec`): verified HEAD = `5e665ec5911fa2a118de70b498b8f0e2841d50ba` and tree = `e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba`, then locked read-only. The main working tree stays clean.
4. **Git-private locator + pending request** persisted as mode-0600, no-follow, single-line-JSON+LF objects under the primary Git directory (`.git/f20-native-uat/<sha>/{locator,request}`). The request (`kind:request`, `status:pending`) binds the candidate commit/tree/ref + runner name + `f20-image` label + a fresh nonce, and is validated by the helper's own `validateRequest`; re-reading through the no-follow reader round-trips clean.

## Immutable pending tuple (for 20-18)

- candidate/commit `5e665ec5911fa2a118de70b498b8f0e2841d50ba`, tree `e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba`
- ref `refs/f20-native-uat/5e665ec5911fa2a118de70b498b8f0e2841d50ba`
- runner `f20-macos-ephemeral-8292012c-e803-4c34-b533-fd83e5fc0525` (id 23)
- image label `f20-image-b10cf631db3207cb2f3402f71600aeea406e94caa8f041bcc9cce8f4545d6c0f`
- nonce `c3b81287aec540c86ca4a632d371324c`
- request digest `6ed9642c…`, locator digest `8ecc2fa6…`

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
