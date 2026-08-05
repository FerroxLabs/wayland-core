---
phase: 20-transactional-delegated-mutation
plan: "31"
subsystem: native-crossaudit
status: complete
disposition: PASS
tags: [cross-audit, pre-native-gate, non-author, schema-validated, wcore-sandbox, renameat, toctou, safe-directory, native-deferred]

# Dependency graph
requires:
  - phase: "20-30"
    provides: "Sealed repaired-successor candidate 17412cf2 (tree 00e41519) + Hetzner locked-build & aggregate 11509/0/48 seal receipts"
provides:
  - "Fresh non-author schema-validated pre-native cross-audit of the sealed successor 17412cf2 at every severity — zero findings, native deferred, admits the 20-32 native-proof re-dispatch"
affects: ["20-32", "20-33", "20-34", "20-35"]

key-links:
  - from: .planning/phases/20-transactional-delegated-mutation/20-31-CROSS-AUDIT.md
    to: .planning/phases/20-transactional-delegated-mutation/20-30-SUMMARY.md
    via: "source/review-base/review authority tuple over the sealed repaired-successor SHA"
    pattern: "source_sha"

sealed_candidate:
  source_sha: 17412cf2f6a8be9d2ec7272f6693f998db4ba2e5
  source_tree: 00e41519ac6782b05e610fcf7fafc772d5040a5d
  predecessor_sha: 95c81ec6a351ec22125497333739fa7c93a0cd8b

review:
  review_sha: 47f5d61b859edf52c372c202ccf8d16e063187ac
  reviewer_id: wayland-f20-31-crossaudit
  source_executor_id: wayland-f20-native-repair-builder
  profile: f20-native-crossaudit
  disposition: PASS

requirements_completed: []

metrics:
  completed: 2026-07-24
duration: ~20min
---

# Phase 20 Plan 31: Pre-Native Cross-Audit of the Re-Sealed Repaired Successor — Summary

**A fresh non-author reviewer (`wayland-f20-31-crossaudit`, distinct from the repair/seal author `wayland-f20-native-repair-builder`) adversarially cross-audited the EXACT sealed successor `17412cf2` (tree `00e41519`) at every severity across the entire 20-29 repair delta — the two unix `rename_into` methods, the `atomic_write_child` publish unification, and the candidate-workflow `safe.directory` guard. Zero findings at blocker/critical/high/medium/low. Native (`native_macos`/`native_windows`) explicitly DEFERRED to the 20-32 re-dispatch. One schema-validated JSON artifact (`wayland-core.phase20-independent-review.v1`, profile `f20-native-crossaudit`) committed and both automated verifiers pass. VERDICT: PASS — the 20-32 native-proof gate is ADMITTED. No source, test, or workflow file was changed.**

## Reviewer identity & separation

- **Reviewer:** `wayland-f20-31-crossaudit` — a fresh non-author reviewer, NOT the author of 20-29 (the fix) or 20-30 (the seal).
- **Source executor (under review):** `wayland-f20-native-repair-builder` (the 20-29 repair author identity, consistent with the 20-24 cross-audit).
- The schema verifier enforces `reviewer_id != source_executor_id`; separation established and green. Reviewed-source authority (`gsd-reviewed-source-20-31`) binds to the sealed successor `17412cf2`/`00e41519`.

## Reviewed candidate (verified, exact sealed tree)

- `source_sha = 17412cf2f6a8be9d2ec7272f6693f998db4ba2e5`; `git rev-parse 17412cf2^{tree} == 00e41519ac6782b05e610fcf7fafc772d5040a5d` — matches the tree recorded by `20-30-SUMMARY.md`.
- `17412cf2` is an ancestor of the working HEAD; `git diff --name-only 17412cf2..HEAD` = exactly the two docs commits (`20-29-SUMMARY.md`, `20-30-SUMMARY.md`) — **no buildable-surface drift since the seal**.
- Functional delta vs the predecessor `95c81ec6`: EXACTLY the 3 declared files (`nightly-windows-soak.yml` +17/-0, `directory_authority.rs` +161/-33, `directory_authority_file.rs` +33/-2); no `Cargo.toml`/`Cargo.lock` touched, no new dependency, no stray production code.

## All-severity disposition (per claimed reviewer: 1 reviewer, 1 artifact)

- **BLOCKER: 0.** Compiles (macOS E0599 closed at 20-29 via the sanctioned `cargo check --tests`; Hetzner clippy `-D warnings` clean; aggregate 11509/0/48).
- **CRITICAL: 0** (T-20-31-01 isolation). The unix `rename_into` resolves BOTH source and destination names ONLY through the retained parent dirfd (`openat`/`renameat` on `handle.as_raw_fd()` with `O_NOFOLLOW`), never `display_path`/the ambient namespace; re-proves source identity against the held object before renaming; no-replace fails closed (`renameat2 RENAME_NOREPLACE` Linux / `renameatx_np RENAME_EXCL` Apple; other-unix `PolicyNotSupported`). `validate_child_name` (exactly one `Component::Normal`) rejects `.`/`..`/absolute/multi-component/empty. No TOCTOU window beyond the pre-existing crate-wide `open_child + identity-token` pattern.
- **HIGH: 0.** `atomic_write_child` unification is byte-for-byte behavior-preserving (`replace=true` overwrite = the historical inline `renameat`; the added identity re-proof rejects nothing legitimate — proven by `wcore-sandbox` 100-passed + aggregate 11509/0/48). The unix file variant is genuinely used (not dead code); the directory variant is `pub` and API-reachable; the Windows `windows::rename_*_into` delegation is unchanged.
- **MEDIUM: 0.** The workflow `safe.directory '*'` guard is scoped to ONLY the two candidate jobs (both gated `f20_candidate == 'true'`), placed after checkout and before the `git rev-parse EXPECTED_COMMIT^{tree}` resolve; the non-candidate `windows-2022` soak job does not receive it. Candidate gating, self-hosted labels, `contents:read` permissions, and no-secrets posture are unchanged — only the intended dubious-ownership abort is bypassed (accepted disposition T-20-29-03: single-tenant Sean-owned runners, no secrets, no untrusted checkout).
- **LOW: 0.** cfg convention consistent; non-unix-non-windows fails closed; SAFETY comments present; NUL handling correct; durability boundary preserved (dir syncs parent, file caller owns the flush).

## Evidence integrity (20-30 receipts bind to the exact sealed SHA/tree)

- 20-30 ran all three proofs against a **detached checkout of `17412cf2`** so the remote-cargo land-gate harness verified the materialized tree == `00e41519` before cargo: locked `--workspace --all-features` build EXIT=0 with **no** lock-update line (Cargo.lock consistent, 1015 packages, byte-identical to the 20-23 seal), plain all-features build EXIT=0, aggregate `nextest --profile ci` run `961262a4-8133-471b-866b-43fbc5c662f0` = **11509 / 0 / 48** (exact prior-seal baseline, honesty gate untripped).
- 20-29 closed the macOS `E0599` via the sanctioned Mac `cargo check -p wcore-sandbox --features live-docker --tests` and Hetzner clippy `-p wcore-sandbox --all-targets --all-features -D warnings` = 0 warnings + `nextest -p wcore-sandbox` = 100 passed.
- No Cargo was run on the Mac by this cross-audit (git-level inspection + recorded receipts only), per the plan constraint.

## Native deferral (NOT passed from source)

`native_macos` and `native_windows` are classified **DEFERRED to the 20-32 native-proof re-dispatch**. The decoy-planted `macos_retained_parent_rename_delete_enumeration_and_cwd_stay_handle_relative` runtime isolation proof and the `SEANDESKTOP` dubious-ownership clearance are hardware facts observable only on the native runners at 20-32 — they are explicitly NOT claimed here. The profile `f20-native-crossaudit` carries `deferred: [native_macos, native_windows]` accordingly.

## Artifact & verifier receipts

- **Artifact:** `.planning/phases/20-transactional-delegated-mutation/20-31-CROSS-AUDIT.md` — one schema-validated JSON object (`wayland-core.phase20-independent-review.v1`), committed at `47f5d61b`. The review JSON does NOT embed its own review SHA.
- **Verify 1 (task scope):** `verify-task-scope.sh <gsd-task-base-20-31> 20-31-CROSS-AUDIT.md` -> `scope-ok base=978478cb generation=g-d28aa5e6… paths=1`.
- **Verify 2 (schema):** `verify-review-result.mjs 47f5d61b 20-31-CROSS-AUDIT.md 17412cf2 00e41519 f20-native-crossaudit` -> `review-result-ok profile=f20-native-crossaudit source=17412cf2 review=47f5d61b reviewer=wayland-f20-31-crossaudit`.

## 20-32 admission decision

**ADMITTED.** A fresh non-author reviewer cross-audited the exact sealed successor at every severity with **zero findings**, native is explicitly deferred, and the schema verifier is green. Per the verdict discipline (PASS only with zero findings + native deferred + verifier green), the 20-32 native-proof re-dispatch gate is admitted with a fresh Sean authorization.

## Explicit non-claims

- **No Phase-20 requirement is marked complete** (terminal claim is 20-35). This plan admits no requirement.
- No native RUNTIME PASS is claimed for either platform; those are 20-32.
- No source/test/workflow change; the only file written is the cross-audit artifact (+ this summary). No push, no merge — work is on `plan/f20-unified-audit-repair` in the ferrox clone only.

## Self-Check: PASSED

- Artifact `.planning/phases/20-transactional-delegated-mutation/20-31-CROSS-AUDIT.md` — FOUND, parses as one JSON object, committed at `47f5d61b`.
- Review commit `47f5d61b` — FOUND on `plan/f20-unified-audit-repair`; changed exactly the one artifact path.
- Sealed SHA/tree `17412cf2`/`00e41519` — verified; both automated verifiers green.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
*Pre-native cross-audit PASS at every severity; native deferred to 20-32; 20-32 native-proof gate ADMITTED; no requirement claimed.*
