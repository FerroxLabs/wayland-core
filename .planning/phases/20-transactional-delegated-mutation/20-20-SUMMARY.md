---
phase: 20-transactional-delegated-mutation
plan: "20"
subsystem: testing
tags: [native-repair, appcontainer, windows, sandbox-boundary, dispatch-smoke, construction-only]

# Dependency graph
requires:
  - phase: "20-19"
    provides: Fresh drop-deny-only-SIDs read-boundary fix whose isolation this plan proves; pristine source base be84bd2
provides:
  - Windows exit-code test-bug fix (choice.exe index -> granted-read exit 0) in live_fs_acl.rs (REQ-native-r5)
  - Windows-portable directory-rename restructure in dispatch_smoke.rs (parent-container rename, not self-held repo) (REQ-native-r6)
  - Two falsifiable AppContainer isolation proofs (genuine DENY still blocks; normal-SID-only grant still denied) (REQ-native-r2)
  - Strengthened granted-read/revoke proof (grant ACE present-during AND absent-after)
affects: ["20-25", "20-16-review"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Stdin-free exit-0 sandbox hold: `type/echo ... && (choice /T N /D Y >nul & ver >nul)` so script exit reflects the granted read, not choice's selection index"
    - "Windows-portable directory-identity swap in tests: rename the NON-self-held parent container (open descendants tolerated via FILE_SHARE_DELETE) instead of the self-held repo, mirroring the in-tree failed_transaction_cleanup_remains_retryable pattern"
    - "Falsifiable AppContainer isolation proofs: negative cases (DENY-wins, normal-SID-only-denied) that FAIL if the boundary regresses"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-20-SUMMARY.md
  modified:
    - crates/wcore-sandbox/tests/live_fs_acl.rs
    - crates/wcore-swarm/tests/dispatch_smoke.rs

key-decisions:
  - "type_and_hold/echo_temp_and_hold keep the proven stdin-free `choice` delay (choice tolerates the sandbox's null stdin; timeout.exe does not) but append `& ver >nul` so the script exits 0 on success; `type` is gated with `&&` so a denied read still surfaces its non-zero exit."
  - "dispatch_smoke replacement tests rename the parent CONTAINER (tmp/box), which the swarm does not hold open on itself, rather than the self-held repo (which has open .swarm-worktrees descendants and panics with Os code 5 on Windows). The retained repo handle survives the move, so a fresh directory recreated at the same repo path is the exact same-path/different-object condition validate_repo_authority rejects."
  - "echo_temp_and_hold was fixed alongside type_and_hold (Rule 1) because it carried the identical choice-exit-1 bug and its consumer (twenty_concurrent_executions_have_unique_temp_roots) asserts exit 0 — leaving it broken would still fail the Windows proof at 20-25 for the exact reason r5 exists."

patterns-established:
  - "Native #[cfg(windows)]/#[ignore] acceptance tests gate on WAYLAND_SANDBOX_LIVE_WINDOWS + AppContainerBackend::is_available() and self-qualify (not skip) on a real host; NATIVE_ACCEPTANCE_CASES + a zero-execution guard keep the count honest."

requirements-completed: []

# Coverage metadata
coverage:
  - id: D1
    description: "type_and_hold/echo_temp_and_hold hold primitive is stdin-free and exit-0-on-granted-read; leak test gates on the granted read (REQ-native-r5)"
    requirement: "REQ-native-r5"
    verification:
      - kind: unit
        ref: "crates/wcore-sandbox/tests/live_fs_acl.rs#one_execution_grant_never_leaks_to_another_identity (native, #[ignore])"
        status: unknown
    human_judgment: true
    rationale: "Native #[cfg(windows)] test; cannot compile or run on Linux. Real green is proven only on the self-hosted msvc AppContainer host at the 20-25 gate."
  - id: D2
    description: "dispatch_smoke different-head + same-head repository replacement rejections are Windows-portable (no rename of an open self-held directory) (REQ-native-r6)"
    requirement: "REQ-native-r6"
    verification:
      - kind: integration
        ref: "crates/wcore-swarm/tests/dispatch_smoke.rs#dispatch_rejects_{different,same}_head_repository_replacement"
        status: pass
    human_judgment: true
    rationale: "Both tests PASS on Hetzner Linux nextest (they run cross-platform), proving no Linux regression and that the identity-change rejection still fires. Windows-portability of the parent-container rename is confirmed only on real Windows at 20-25."
  - id: D3
    description: "AppContainer isolation preserved after dropping deny-only SIDs: genuine DENY still blocks a granted read; normal-SID-only grant still denied (REQ-native-r2)"
    requirement: "REQ-native-r2"
    verification:
      - kind: unit
        ref: "crates/wcore-sandbox/tests/live_fs_acl.rs#{deny_ace_still_blocks_granted_read,normal_sid_only_grant_is_denied} (native, #[ignore])"
        status: unknown
    human_judgment: true
    rationale: "Native #[cfg(windows)] isolation proofs; cannot run on Linux. Falsifiable green is proven only on the self-hosted msvc AppContainer host at 20-25."

# Metrics
duration: ~55min
completed: 2026-07-23
status: complete
---

# Phase 20 Plan 20: Native Test-Bug Fixes + AppContainer Isolation Proofs Summary

**Fixed the two Windows test bugs the hardware run exposed (choice.exe exit-index assumption; non-portable directory rename) and authored two falsifiable AppContainer isolation proofs that the 20-19 read-boundary fix restored reads WITHOUT weakening the sandbox — construction-proven on the Mac and Linux-clean on Hetzner; all native green deferred to 20-25.**

## Candidate Tuple

- **source_sha:** `31844fb1648934360b73f9017e3f11e08b97f345` — tree `f2fe177ab2b760349c0ca01a332c27f82600af03`
- **inherited base (20-19):** `source_sha=0e8e6c1`, tree `409fe8d1`, pristine base `be84bd2` (tree `6d09484`)
- **task_base (scope authority):** captured at `fadf15e12f40a19dc6e5fa304a19bed1f257332c`, generation `g-255922c3…`
- **Touched paths (scope-verified, exactly 2):** `crates/wcore-sandbox/tests/live_fs_acl.rs`, `crates/wcore-swarm/tests/dispatch_smoke.rs`. No production source changed.

## Performance

- **Duration:** ~55 min
- **Completed:** 2026-07-23
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments

- **REQ-native-r5 (exit-code bug):** `type_and_hold` / `echo_temp_and_hold` now hold the sandboxed process alive with a stdin-free `choice` delay followed by `ver` (a builtin that always exits 0), so the script's exit code reflects the granted READ succeeding — not `choice.exe`'s 1-based selection index (default `Y` => exit 1, which can never be 0). `type` is `&&`-gated so a denied read still surfaces its non-zero exit. `one_execution_grant_never_leaks_to_another_identity` now gates on the granting identity's exit 0 AND the bytes actually read.
- **REQ-native-r6 (rename bug):** the different-head and same-head repository-replacement rejection tests are Windows-portable. They rename the non-self-held parent container instead of the swarm-held `repo` directory (which retains open `.swarm-worktrees` descendant handles and so panics with `Os { code: 5, PermissionDenied }` on Windows). The retained `repo` handle survives the parent move, so a fresh directory recreated at the same `repo` path is the exact same-path/different-object condition `validate_repo_authority` rejects. Both tests PASS on Hetzner Linux.
- **REQ-native-r2 (isolation preserved):** two new falsifiable `#[cfg(windows)]`/`#[ignore]` proofs — `deny_ace_still_blocks_granted_read` (an explicit DENY ace blocks the child even with a matching package-SID grant) and `normal_sid_only_grant_is_denied` (a file granted only to `Everyone`/`S-1-1-0` with no package-SID grant is still denied). Both FAIL if the boundary regresses. `granted_path_is_readable_then_revoked` was strengthened to prove the grant ACE is present DURING the run (not only absent after), making the revoke falsifiable.

## Task Commits

1. **Task 1: Fix the exit-code and directory-rename test bugs** — `629c810e` (test)
2. **Task 2: Author the AppContainer isolation acceptance proofs** — `31844fb1` (test)

Both were committed atomically via `git apply --cached` hunk-splitting so each task is an independent, internally-consistent commit even though both touch `live_fs_acl.rs`.

## Files Created/Modified

- `crates/wcore-sandbox/tests/live_fs_acl.rs` — fixed both `choice.exe` hold primitives (exit 0 on granted read); strengthened `granted_path_is_readable_then_revoked` (present-during + absent-after); gated the leak test on the granted read; added `deny_ace_still_blocks_granted_read` and `normal_sid_only_grant_is_denied`; bumped `NATIVE_ACCEPTANCE_CASES` 9 -> 11 and its zero-execution guard.
- `crates/wcore-swarm/tests/dispatch_smoke.rs` — added `replace_repo_container` helper; restructured both repository-replacement tests to rename the parent container.

## Gate Results

### Mac construction (per-task, allowed operations only)
- **Scope (`verify-task-scope.sh`):** `scope-ok base=fadf15e1 generation=g-255922c3… paths=2` — exactly the two declared paths, no out-of-scope drift.
- **Format (`vx rustfmt --edition 2024 --check`):** clean for both files. (The plan's verify literals hardcode `--edition 2021`; the workspace is edition 2024, so 2024 is the authoritative fmt — confirmed by the Hetzner `cargo fmt` below. This matches the 20-19 edition note.)

### Hetzner Linux (committed-HEAD authoritative, build-clone HEAD `31844fb1`)
- **`vx cargo fmt --all --check`:** clean (EXIT=0).
- **`vx cargo clippy -p wcore-sandbox -p wcore-swarm --all-targets --all-features -- -D warnings`:** **EXIT=0**.
- **`vx cargo nextest run -p wcore-sandbox -p wcore-swarm`:** **235 passed, 13 skipped, 0 failed (EXIT=0)**. The 13 skips are the `#[ignore]` native/live cases (Windows AppContainer + live-bwrap), which do not run on this Linux host by design.
- **Restructured replacement tests specifically:** `dispatch_rejects_same_head_repository_replacement` and `dispatch_rejects_different_head_repository_replacement` both **PASS** on Linux — the parent-container-rename restructure still produces the `directory identity changed` rejection.

Note: the known-slow `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` timeout is out of scope here — this plan tests `wcore-sandbox` + `wcore-swarm` only, so that `wcore-agent` test never runs.

## Linux-proven vs. deferred-to-20-25

- **Linux-proven here:** no Linux regression — `wcore-sandbox` + `wcore-swarm` are fmt-clean, clippy-clean at `-D warnings`, and full-green on nextest. The two `dispatch_smoke` replacement-rejection tests (which run cross-platform) PASS on Linux with the portable parent-container restructure, confirming the identity-change rejection still fires and the restructure did not regress the security property. Scope and diff hygiene enforced (exactly 2 declared files).
- **UNPROVEN here, deferred to the native-proof gate (20-25):** every Windows runtime claim. The `live_fs_acl.rs` file is `#![cfg(windows)]`, so Linux compiles nothing in it. Real green for the exit-0 hold primitive (r5), the `deny_ace_still_blocks_granted_read` / `normal_sid_only_grant_is_denied` isolation proofs (r2), the strengthened grant/revoke case, AND the Windows-portability of the `dispatch_smoke` parent-container rename (r6) are all proven ONLY on the self-hosted msvc AppContainer host at 20-25. No native claim is made from Linux.

## Decisions Made

- Kept `choice.exe` as the hold delay (it is the proven stdin-free primitive under the sandbox's null stdin; `timeout.exe` fails under redirected stdin) and appended `& ver >nul` for a deterministic exit-0, rather than swapping in `ping -n` (needs a network capability under AppContainer) or a busy-wait (CPU-heavy, timing-unreliable). See BRIEF §7.2.
- Restructured the rename rather than trying to drop the swarm's handles: the swarm's in-memory `repo_authority` handle IS the identity-change detector and must stay alive, so dropping it is not an option. Renaming the non-self-held parent container is the portable path that keeps the swarm — and its rejection — intact. The in-tree, non-cfg-gated `worktree_tests::failed_transaction_cleanup_remains_retryable` establishes this ancestor-rename pattern as Windows-portable.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed `echo_temp_and_hold` exit code alongside `type_and_hold`**
- **Found during:** Task 1
- **Issue:** The plan action names only `type_and_hold`, but `echo_temp_and_hold` carries the identical `choice.exe` exit-1 bug, and its consumer `twenty_concurrent_executions_have_unique_temp_roots` asserts `exit_code == 0`. Leaving it unfixed would still fail the Windows proof at 20-25 for the exact reason r5 exists.
- **Fix:** Applied the same `& ver >nul` exit-0 suffix to `echo_temp_and_hold`. Same declared file (`live_fs_acl.rs`), same bug class.
- **Verification:** fmt-clean; scope-clean (still exactly the 2 declared files); hardware green deferred to 20-25.
- **Committed in:** `629c810e` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug). **Impact on plan:** In-scope (declared file), same root-cause bug as the named fix, no scope creep — makes the eventual 20-25 hardware proof honest.

## Issues Encountered

- SSH to Hetzner dropped once mid-`nextest` (network timeout). Retried; the persistent build-clone target cache made the retry fast and it completed green. No source change involved.

## Requirements & Claims

No Phase 20 requirement is marked complete by this plan. REQ-native-r2/r5/r6 exist as reviewable, gated, falsifiable native tests plus a Linux-proven no-regression result. No aggregate, native, requirement, or phase claim is made. The candidate proceeds to the native-proof gate (20-25) and the fresh 20-16 review.

## Next Phase Readiness

- Ready for the next construction plan in the serial set (20-21) and, ultimately, the 20-25 native-proof gate where the Windows behavior of these tests is empirically confirmed on the self-hosted msvc AppContainer runner.
- One residual to confirm at 20-25: the `dispatch_smoke` parent-container rename is portable-by-construction (mirrors an in-tree Windows-passing pattern) but its Windows green is only asserted, not yet observed on hardware.

## Self-Check: PASSED

- `crates/wcore-sandbox/tests/live_fs_acl.rs` — FOUND (both `choice` holds fixed; `deny_ace_still_blocks_granted_read` + `normal_sid_only_grant_is_denied` present; count 9 -> 11)
- `crates/wcore-swarm/tests/dispatch_smoke.rs` — FOUND (`replace_repo_container` helper; both replacement tests restructured)
- Task 1 commit `629c810e` — FOUND in `git log`
- Task 2 commit `31844fb1` — FOUND in `git log`
- Hetzner Linux at HEAD `31844fb1`: fmt EXIT=0, clippy `-D warnings` EXIT=0, nextest 235 passed / 0 failed; both replacement tests PASS on Linux.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-23*
