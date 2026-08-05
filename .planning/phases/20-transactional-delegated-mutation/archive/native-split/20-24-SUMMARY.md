---
phase: 20-transactional-delegated-mutation
plan: "24"
subsystem: review
tags: [native-repair, cross-audit, pre-native-gate, appcontainer, deny-only-sid, evidence-integrity, schema-validated, non-author-review]

# Dependency graph
requires:
  - phase: "20-23"
    provides: "The sealed repaired candidate SHA 95c81ec6 (tree 784f4980) with the consistent Cargo.lock + Hetzner 11509/0/48 aggregate receipt"
provides:
  - "A fresh non-author schema-validated cross-audit of the exact sealed candidate at every severity (BLOCKER/CRITICAL/HIGH/MEDIUM/LOW = 0), native explicitly deferred to 20-25 (REQ-native-r12 pre-native gate)"
  - "A per-reviewer on-disk schema-validated artifact (wayland-core.phase20-independent-review.v1, profile f20-native-crossaudit) — closes the 20-08/20-16 attestation gap where claimed reviews had no artifact (REQ-native-r13)"
  - "A zero-finding PASS that admits the 20-25 native-proof gate"
affects: ["20-25", "20-26"]

tech-stack:
  added: []
  patterns:
    - "Pre-native cross-audit binds to the exact sealed SHA via a captured reviewed-source authority object (gsd-reviewed-source-20-24 = 95c81ec6/784f4980) separate from the scope-authority task base (gsd-task-base-20-24 = review-commit-base 43c42916); the schema verifier re-derives source^{tree} so the asserted tree cannot be decoupled from the asserted commit"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-24-CROSS-AUDIT.md
    - .planning/phases/20-transactional-delegated-mutation/20-24-SUMMARY.md
  modified: []

key-decisions:
  - "PASS with ZERO findings at every severity. The critical EoP surface (deny-only SID drop in windows_impl/process.rs) was adversarially re-confirmed at the source: CreateRestrictedToken drops only SidsToDisable (0/null), process creation uses restricted_token (never current_token / full primary), Low-IL is set on the restricted token AND re-asserted post-spawn, the AppContainer package SID capability is preserved, and no WCORE_DIAG_TOKEN / diagnostic token swap survives in the sealed tree. Containment is intrinsic to the AppContainer lowbox model, and the load-bearing falsifiable proof normal_sid_only_grant_is_denied encodes exactly that invariant."
  - "native_macos / native_windows are classified DEFERRED to 20-25 — not passed from source. Every macOS/Windows RUNTIME claim across 20-19..20-23 is explicitly gated to the self-hosted msvc AppContainer runner and the Scaleway Apple-silicon runner; this cross-audit prosecutes source, tests, harness, and evidence integrity only, under the f20-native-crossaudit profile whose deferred set is [native_macos, native_windows]."
  - "Receipt-to-SHA binding was confirmed (not re-run). The 20-23 remote-cargo land-gate harness verifies the extracted tree hash == committed HEAD tree 784f4980 before cargo runs, and the aggregate 11509/0/48 (run 1ab1110b) + the --locked build EXIT=0 were produced against that exact tree; 95c81ec6^{tree} == 784f4980 was independently confirmed. No Cargo ran on the Mac."

requirements-completed: []

coverage:
  - id: X1
    description: "Fresh non-author reviewer cross-audits the exact sealed candidate SHA at every severity with zero findings before the Sean-gated native run (REQ-native-r12)"
    requirement: "REQ-native-r12"
    verification:
      - kind: automated
        ref: "node verify-review-result.mjs 25c3c484 20-24-CROSS-AUDIT.md 95c81ec6 784f4980 f20-native-crossaudit -> review-result-ok (all_severity/evidence_integrity/integration_authority PASS; findings all 0; disposition PASS; reviewer wayland-f20-24-crossaudit != source wayland-f20-native-repair-builder)"
        status: pass
      - kind: automated
        ref: "bash verify-task-scope.sh $(git-path gsd-task-base-20-24) 20-24-CROSS-AUDIT.md -> scope-ok base=43c42916 paths=1 (the review commit 25c3c484 touched exactly the cross-audit artifact)"
        status: pass
    human_judgment: true
    rationale: "Source/test/harness/evidence prosecuted adversarially; the critical deny-only-SID EoP surface and the harness anti-drift guard were re-read at the sealed tree and found clean. Native runtime is deferred, not claimed."
  - id: X2
    description: "Every claimed reviewer emitted its OWN schema-validated artifact; no prose-only review counted (REQ-native-r13)"
    requirement: "REQ-native-r13"
    verification:
      - kind: automated
        ref: "One reviewer (wayland-f20-24-crossaudit) was run; its single schema-validated artifact 20-24-CROSS-AUDIT.md validates under verify-review-result.mjs. No additional cross-AI reviewer was claimed, so no sibling artifact is asserted."
        status: pass
    human_judgment: false

duration: ~40min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 24: Pre-Native Cross-Audit of the Sealed Candidate Summary

**A fresh non-author reviewer (`wayland-f20-24-crossaudit`) adversarially cross-audited the EXACT sealed candidate `95c81ec6` (tree `784f4980`) at every severity — the deny-only SID drop, the falsifiable isolation and Job-Object proofs, the wrong-OS anti-drift guard, the macOS re-validation, the lock consistency, the runner pin / writer parity / review profiles, and the 11509/0/48 receipt-to-SHA binding — recorded the result as one schema-validated `wayland-core.phase20-independent-review.v1` artifact under profile `f20-native-crossaudit`, and found ZERO findings at BLOCKER/CRITICAL/HIGH/MEDIUM/LOW. Native (macOS/Windows runtime) is explicitly DEFERRED to 20-25. Both automated verifiers are green. Disposition: PASS — the 20-25 native-proof gate is ADMITTED. No source or test file was changed; no Phase-20 requirement was completed.**

## Reviewer Identity & Non-Author Separation

- **Reviewer:** `wayland-f20-24-crossaudit` — authored none of the 20-19..20-23 source/test/harness commits; this is a fresh, adversarial, source-changing-nothing review.
- **Source executor (under review):** `wayland-f20-native-repair-builder` — the native-repair delta author whose sealing commit is `95c81ec6`.
- The two identities are distinct and stable; the schema verifier rejects a review where `reviewer_id == source_executor_id`. Separation established → NOT failed closed.

## Sealed Candidate Tuple (verified)

- **Sealed source_sha:** `95c81ec6a351ec22125497333739fa7c93a0cd8b`
- **Sealed source_tree:** `784f498002b9944856aedee6cb3db347b55c1dcc` (independently confirmed `95c81ec6^{tree} == 784f4980`)
- **Review commit:** `25c3c48483cb1bec407361ddecb66d735086dd09` (adds only `20-24-CROSS-AUDIT.md`; its own SHA is NOT embedded in the artifact)
- **Scope authority (gsd-task-base-20-24):** captured at HEAD `43c42916`, tree `6540d32d`, generation `g-76b6767c…`
- **Reviewed-source authority (gsd-reviewed-source-20-24):** `95c81ec6` / `784f4980` (read-back exact)
- **HEAD context:** `43c42916` = the sealed candidate + one docs-only commit adding `20-23-SUMMARY.md`; working tree clean at capture.

## Per-Reviewer Artifact Identity

| Reviewer | Artifact | Profile | Disposition | Findings (B/C/H/M/L) |
|----------|----------|---------|-------------|----------------------|
| `wayland-f20-24-crossaudit` | `20-24-CROSS-AUDIT.md` | `f20-native-crossaudit` | PASS | 0 / 0 / 0 / 0 / 0 |

No additional cross-AI reviewer was claimed, so no sibling `20-24-CROSS-AUDIT-<reviewer>.md` is asserted. Per R13, only schema-validated on-disk artifacts count; the single artifact above is the only claim.

## All-Severity Prosecution (what was adversarially re-confirmed at the sealed tree)

1. **Deny-only SID drop — critical EoP surface (CLEAN).** `windows_impl/process.rs execute_blocking` now passes `0, ptr::null_mut()` for `SidsToDisable`. `CreateProcessAsUserW` is invoked with `restricted_token.as_raw()` — NEVER `current_token` / the full primary token. `DISABLE_MAX_PRIVILEGE` is preserved; the explicit Low-IL label is set on the restricted token and re-asserted by a post-spawn OS-layer invariant that bails if the child is not Low IL; `SECURITY_CAPABILITIES.AppContainerSid` is preserved; the Job Object retains `KILL_ON_JOB_CLOSE` + `ActiveProcessLimit` + breakaway-deny. `git grep WCORE_DIAG_TOKEN` over the sealed tree returns nothing — the boundary-breaking diagnostic is gone. The boundary was NOT widened.
2. **Isolation proofs are genuinely falsifiable.** `normal_sid_only_grant_is_denied` (Everyone/S-1-1-0 grant with no package-SID grant → child STILL denied) is the load-bearing proof that the SID drop did not weaken the sandbox; `deny_ace_still_blocks_granted_read` proves DENY-before-ALLOW; the grant/revoke case asserts present-during AND absent-after. `NATIVE_ACCEPTANCE_CASES=11` with a zero-execution guard keeps the count honest.
3. **Job-Object tests exercise the real mechanism.** `hard_process_containment_windows.rs` (5 `#![cfg(windows)]`/`#[ignore]` cases) drives the actual Job Object through host-side CIM/tasklist liveness queries (running mid-flight → GONE after job close), an active-process-cap fan-out bound, breakaway denial, and a black-box preflight through the public `SandboxBackend` trait — containment assertions, not parent-exit tautologies.
4. **Harness repoint + anti-drift guard cannot map a native target to a wrong-OS test.** `Assert-TargetOsGate` (ps1) and `assert_target_os_gate` (macOS bash) require a positive own-OS cfg gate AND no foreign-OS cfg on each OS-specific target's selected source; the Linux-only Bubblewrap test fails the positive gate closed. The ps1 also verifies `expectedCommit`/`expectedTree` before cargo, binding any proof run to the exact sealed tree.
5. **macOS re-validation removed aspirational mappings.** All 8 macOS targets resolve to existing tests; the two macOS-specific ones resolve to `#![cfg(target_os = "macos")]` sources selected filename-independently by defining function.
6. **Lock is consistent.** The `be84bd2..95c81ec6` delta touches `Cargo.lock` only among manifests (+7/-0, 0 new `[[package]]` stanzas); the Hetzner `--locked` build returned EXIT=0 (no "needs updating").
7. **Runner pin / writer parity / review profiles are sound.** The Windows candidate leg is pinned to `[self-hosted, Windows, X64, msvc]`; the UAT-proof writer persists exactly the tuple the verifier reads; `verify-review-result.mjs` `f20-native-crossaudit` = `{all_severity, evidence_integrity, integration_authority}` PASS with `deferred [native_macos, native_windows]`, and `f20-09/11/14/15/16` are unchanged.
8. **Receipt-to-SHA binding confirmed.** The 20-23 remote-cargo harness verifies extracted tree == committed `784f4980` before cargo; the aggregate `11509 passed / 0 failed / 48 skipped` (run `1ab1110b`) and `--locked` build EXIT=0 were produced against that exact tree.

## Native Gate Disposition

- **native_macos:** DEFERRED to 20-25 (Scaleway Apple-silicon runner).
- **native_windows:** DEFERRED to 20-25 (self-hosted msvc AppContainer runner).
- Neither is passed from source; both remain in the `deferred` set of the `f20-native-crossaudit` profile. This cross-audit makes NO native runtime claim.

## Automated Verification (both green)

- `bash verify-task-scope.sh $(git-path gsd-task-base-20-24) 20-24-CROSS-AUDIT.md` → `scope-ok base=43c42916 generation=g-76b6767c… paths=1`.
- `node verify-review-result.mjs 25c3c484 20-24-CROSS-AUDIT.md 95c81ec6 784f4980 f20-native-crossaudit` → `review-result-ok profile=f20-native-crossaudit source=95c81ec6 review=25c3c484 reviewer=wayland-f20-24-crossaudit`.

No Cargo ran on the Mac. The only files written are the cross-audit artifact and this summary.

## Explicit Non-Claims

- No macOS/Windows native RUNTIME green is claimed — that is 20-25's gate.
- No Phase 20 requirement is marked complete (REQ-native-r12/r13 have their pre-native evidence here; completion stays deferred to 20-28).
- No push, no merge; work committed only on `plan/f20-unified-audit-repair` in the ferrox clone.

## Verdict

**PASS — zero findings at every severity, native explicitly deferred, schema verifier green. The 20-25 native-proof gate is ADMITTED.**

## Self-Check: PASSED

- `.planning/phases/20-transactional-delegated-mutation/20-24-CROSS-AUDIT.md` — FOUND; committed at review commit `25c3c484`; validates under `verify-review-result.mjs` profile `f20-native-crossaudit`.
- Review commit `25c3c484` — FOUND in `git log` (branch `plan/f20-unified-audit-repair`); touched exactly the cross-audit artifact (`scope-ok paths=1`).
- Reviewed-source authority reads back `95c81ec6` / `784f4980` — the exact sealed candidate.
- Reviewer `wayland-f20-24-crossaudit` != source executor `wayland-f20-native-repair-builder` — non-author separation established.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
*Cross-audit PASS at the sealed candidate `95c81ec6`; native deferred to 20-25; no source/test changed; no requirement completed.*
