---
phase: 20-transactional-delegated-mutation
plan: "29"
subsystem: infra
tags: [wcore-sandbox, renameat, renameat2, renameatx_np, directory-authority, github-actions, safe.directory, toctou]

# Dependency graph
requires:
  - phase: 20-transactional-delegated-mutation
    provides: "20-25 sealed candidate 95c81ec6 (tree 784f4980) + its retained RED native-UAT evidence identifying the two exact RED causes"
provides:
  - "Cross-platform DirectoryAuthority::rename_into and RegularFileAuthority::rename_into with a #[cfg(unix)] handle-relative renameat implementation (was #[cfg(windows)]-only), closing the macOS lib-test E0599"
  - "Handle-relative no-replace rename primitives (renameat2 RENAME_NOREPLACE on Linux, renameatx_np RENAME_EXCL on Apple) preserving the Windows replace=false fail-closed semantics"
  - "atomic_write_child unix publish unified through the file rename_into (behaviour-preserving), making the unix file variant genuinely used (not dead code under -D warnings)"
  - "safe.directory git guard on both f20 candidate jobs in nightly-windows-soak.yml, before the tree-resolve, unblocking the self-hosted SEANDESKTOP msvc runner"
  - "New re-sealable successor candidate 17412cf2 (tree 00e41519) over the sealed 95c81ec6 — exactly the two RED-cause fixes and nothing else"
affects: [20-30, 20-31, 20-32, 20-33, 20-34, 20-35]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Unix handle-relative rename: resolve source+destination names ONLY through the retained destination-parent dirfd, re-prove source identity against the held object, then renameat/renameat2/renameatx_np"
    - "OS no-replace primitive selection by cfg (linux renameat2 RENAME_NOREPLACE vs apple renameatx_np RENAME_EXCL) with a fail-closed PolicyNotSupported for other unix targets"

key-files:
  created: []
  modified:
    - crates/wcore-sandbox/src/directory_authority.rs
    - crates/wcore-sandbox/src/directory_authority_file.rs
    - .github/workflows/nightly-windows-soak.yml

key-decisions:
  - "Followed the existing per-method cfg convention (#[cfg(unix)] / #[cfg(windows)] / #[cfg(not(any(unix,windows)))]) rather than a single #[cfg(not(windows))] libc branch, so the crate still compiles on exotic non-unix-non-windows targets and matches surrounding methods"
  - "Routed the unix atomic_write_child publish through temporary_authority.rename_into(self, name, true) (replace=true = overwrite) to keep it byte-for-byte behaviour-preserving while making the unix file rename_into genuinely used"
  - "Used safe.directory value '*' (not an exact path) per the plan threat model T-20-29-03 — single-tenant Sean-owned runners, contents:read, no secrets; avoids a backslash/forward-slash path mismatch burning another scarce native run"
  - "Advanced the task-scope base between tasks via --start-fresh so each task's per-task scope gate (Task 1 = 2 sandbox files, Task 2 = workflow only) passes exactly as the plan's verify blocks are written"

patterns-established:
  - "Handle-relative unix rename that re-proves source identity before renaming (TOCTOU mitigation mirroring open_child_directory + identity-token checks)"

requirements-completed: []  # NO Phase-20 requirement is claimed here (terminal claim is 20-35). r9/r4/r11 are advanced (compile/lint/Linux-runtime), NOT completed — hardware runtime proof is deferred to 20-32.

coverage:
  - id: D1
    description: "Unix DirectoryAuthority::rename_into + RegularFileAuthority::rename_into compile; the cfg(all(test, unix)) macOS acceptance module (directory_authority_tests.rs:149) compiles, closing E0599 (r9 compile leg)"
    requirement: "REQ-native-r9"
    verification:
      - kind: other
        ref: "cargo check -p wcore-sandbox --features live-docker --tests (THIS Mac, Darwin arm64) — Finished, E0599 closed; only pre-existing unrelated macOS-only dead-code warning (process_tree.rs:403 signal)"
        status: pass
    human_judgment: true
    rationale: "Compile is proven here; the ISOLATION property (rename lands on the retained object, never a decoy; replace=false fails closed) is only proven at runtime by the decoy-planted macOS acceptance test on hardware at 20-32."
  - id: D2
    description: "Linux is unregressed: the newly-used unix file rename_into is not dead code and no clippy -D warnings appear; publish tests still pass (r4)"
    requirement: "REQ-native-r4"
    verification:
      - kind: other
        ref: "Hetzner: clippy -p wcore-sandbox --all-targets --all-features -- -D warnings — Finished, 0 warnings"
        status: pass
      - kind: unit
        ref: "Hetzner: cargo nextest run -p wcore-sandbox — 100 passed, 2 skipped (incl. archive replace_tree/atomic_write_child publish tests: temp-publish re-proof rejects nothing legitimate)"
        status: pass
    human_judgment: false
  - id: D3
    description: "safe.directory git guard added to f20-windows-candidate (required) and f20-macos-candidate (defensive), before the EXPECTED_COMMIT^{tree} resolve, unblocking the self-hosted msvc runner (r11 structural leg)"
    requirement: "REQ-native-r11"
    verification:
      - kind: manual_procedural
        ref: "grep -c 'add safe.directory' = 2; YAML parses (jobs: windows-soak, f20-windows-candidate, f20-macos-candidate); guard at lines 232/281 precede tree-resolve at line 249"
        status: pass
    human_judgment: true
    rationale: "The structural guard is verified; that setup now actually CLEARS the 'dubious ownership' abort on the real self-hosted SEANDESKTOP runner is a hardware fact only observable at the 20-32 native re-dispatch."

# Metrics
duration: ~35min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 29: Repaired-successor fix for the Phase-20 native RED Summary

**Unix `rename_into` (handle-relative `renameat`/`renameat2`/`renameatx_np`, identity re-proven, no-replace fail-closed) closes the macOS E0599, plus a `safe.directory` git guard unblocks the self-hosted Windows candidate runner — one clean successor delta over sealed candidate `95c81ec6`.**

## Performance

- **Duration:** ~35 min
- **Completed:** 2026-07-24T01:36Z
- **Tasks:** 2
- **Files modified:** 3 (source delta over the sealed candidate)

## Candidate identity

- **Predecessor (sealed 20-25 candidate):** `95c81ec6a351ec22125497333739fa7c93a0cd8b` (tree `784f498002b9944856aedee6cb3db347b55c1dcc`)
- **Pre-edit drift check:** the three touched files' blobs were byte-identical to the sealed tree before editing (no source drift) — clean successor confirmed.
- **New candidate `source_sha`:** `17412cf2f6a8be9d2ec7272f6693f998db4ba2e5`
- **New candidate source tree:** `00e41519ac6782b05e610fcf7fafc772d5040a5d`
- **Successor delta vs 95c81ec6 (excluding .planning docs):** exactly the 3 declared files — the two RED-cause fixes and nothing else.

## Accomplishments
- **macOS compile defect fixed (r9 compile leg):** `DirectoryAuthority::rename_into` and `RegularFileAuthority::rename_into` are now single methods with internal cfg branches. `#[cfg(windows)]` delegates to the existing `windows::rename_*_into`; `#[cfg(unix)]` renames a validated single-component child entirely through the retained destination/target-parent dirfd. The `cfg(all(test, unix))` macOS acceptance module now compiles — E0599 at `directory_authority_tests.rs:149` is closed.
- **Isolation preserved, not merely satisfied (T-20-29-01/-02):** the unix rename validates `child_name` and the derived source name as single safe components, resolves BOTH names only through the retained parent handle (never `display_path`), re-proves the source child's identity against the held object before renaming, and uses the OS no-replace primitive so `replace=false` fails closed (`renameat2 RENAME_NOREPLACE` on Linux, `renameatx_np RENAME_EXCL` on Apple; plain `renameat` overwrite for `replace=true`).
- **Unix file variant made genuinely used (r4):** `atomic_write_child`'s unix publish now routes through `temporary_authority.rename_into(self, name, true)` (replace=true overwrite), a byte-for-byte behaviour-preserving unification of the former inline `renameat` — so the unix file `rename_into` is not dead code under `-D warnings`.
- **Windows runner unblocked (r11):** a `git config --global --add safe.directory '*'` step was added to `f20-windows-candidate` (required — the proven RED cause) and `f20-macos-candidate` (defensive), immediately after checkout and before the `git rev-parse EXPECTED_COMMIT^{tree}` resolve.

## Task Commits

Each task was committed atomically:

1. **Task 1: Expose cfg(unix) rename_into on both authorities, isolation preserved** — `135eb7b1` (fix)
2. **Task 2: Add the safe.directory guard to the candidate native job(s) before tree-resolve** — `17412cf2` (ci)

## Files Created/Modified
- `crates/wcore-sandbox/src/directory_authority.rs` — cross-platform `DirectoryAuthority::rename_into` with a `#[cfg(unix)]` handle-relative `renameat` path; new `renameat_child` / `renameat_no_replace` / `retained_child_name` unix helpers; `atomic_write_child` publish unified through the file `rename_into`.
- `crates/wcore-sandbox/src/directory_authority_file.rs` — cross-platform `RegularFileAuthority::rename_into` reusing the same `renameat` machinery with source-identity re-proof; caller owns durability (no internal sync).
- `.github/workflows/nightly-windows-soak.yml` — `safe.directory '*'` guard on both f20 candidate jobs (pwsh on Windows, bash on macOS) before the tree-resolve. `windows-soak` job, candidate gating, self-hosted labels, permissions, and proof invocations unchanged.

## Verification receipts

**Mac construction gates (per task):**
- Scope: Task 1 `scope-ok base=2e57b412 paths=2`; Task 2 `scope-ok base=135eb7b1 paths=1` (base advanced via `--start-fresh` between tasks).
- `vx rustfmt --edition 2024 --check` on both sandbox files → OK (workspace edition is 2024; the plan literal `--edition 2021` was corrected to 2024).
- **The ONE sanctioned Mac cargo invocation (CONTEXT D5 carve-out):** `cargo check -p wcore-sandbox --features live-docker --tests` on THIS Mac (Darwin arm64) → **Finished**; the `cfg(all(test, unix))` acceptance module compiles, **E0599 closed**. Only warning is pre-existing, unrelated, macOS-only dead code (`process_tree.rs:403` `signal`), out of scope. Compile/type-check only — no test run, no Docker.

**Hetzner Linux gates (committed HEAD `17412cf2`):**
- `clippy -p wcore-sandbox --all-targets --all-features -- -D warnings` → **Finished, 0 warnings** (unix file `rename_into` not dead code; no Linux regression).
- `nextest run -p wcore-sandbox` → **100 passed, 2 skipped**. Confirms the plan-checker WARNING: the added source-identity re-proof rejects **nothing legitimate** — the archive `replace_tree`/`atomic_write_child` publish tests (e.g. `directory_authority::archive::tests::crash_after_descendant_removal_recovers_original_before_reads`) and `relative_creation_stays_bound_to_renamed_parent_object` all pass.

## Decisions Made
- Used the file's existing per-method cfg convention (`#[cfg(unix)]` / `#[cfg(windows)]` / `#[cfg(not(any(unix,windows)))]`) rather than a literal `#[cfg(not(windows))]` libc branch; `#[cfg(unix)]` is the platform that had the E0599 (macOS) and this keeps compilation on exotic targets and matches surrounding methods. This is a faithful reading of "one method with internal cfg branches; not(windows) performs the renameat rename" — `#[cfg(unix)] ⊂ #[cfg(not(windows))]`.
- Kept the unix directory `rename_into` syncing the destination parent (matching `windows::rename_directory_into`) but the file `rename_into` NOT syncing (the caller `atomic_write_child` owns the parent flush) — preserving the historical publish durability boundary.
- `safe.directory '*'` over an exact path (threat model T-20-29-03 accept): single-tenant Sean-owned runners, `contents: read`, no secrets, no untrusted checkouts.

## Deviations from Plan

None affecting scope or behaviour. Two literal-command adjustments explicitly sanctioned by the dispatch:
- `vx rustfmt --edition 2021 --check` → `--edition 2024` (workspace edition is 2024).
- Advanced the scope base between tasks with `verify-task-scope.sh --start-fresh` (g0 `g-59ffe58b…` → g1 `g-15bda950…`) so each task's disjoint per-task scope gate passes exactly as written; no source impact.

## Issues Encountered
None. The pre-existing `fix1_dispatch_budget_aborts_with_partial_result` default-profile Linux flake was not encountered (it is not in `wcore-sandbox`); per the plan it is out of scope and was not touched.

## Explicit non-claims / deferrals
- **macOS-compile-proven-here** (E0599 closed via the sanctioned `cargo check --tests`); **runtime rename isolation on macOS/Windows hardware is deferred to 20-32** (the decoy-planted `macos_retained_parent_rename_delete_enumeration_and_cwd_stay_handle_relative` acceptance test runs on hardware there).
- **Windows setup-clearing** (that the self-hosted SEANDESKTOP runner no longer aborts with "dubious ownership" and the proof script actually starts) is a hardware fact deferred to the 20-32 native re-dispatch; only the structural guard is proven here.
- **NO native, aggregate, requirement, or phase claim** is made. REQ-native-r9/-r4/-r11 are advanced (compile / Linux-lint+test / structural), not completed. The terminal requirement claim is 20-35.
- Re-dispatch structure (per the plan's `<redispatch_decision>`): the repaired successor flows through NEW plans 20-30 (re-seal) → 20-31 (pre-native cross-audit) → 20-32 (native re-dispatch) → 20-33 → 20-34 → 20-35; 20-25/20-26/20-27/20-28 are neither re-bound nor edited.

## Next Phase Readiness
- New candidate `17412cf2` (tree `00e41519`) is ready to be re-sealed by 20-30 and re-dispatched to hardware by 20-32 with fresh Sean authorization.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
