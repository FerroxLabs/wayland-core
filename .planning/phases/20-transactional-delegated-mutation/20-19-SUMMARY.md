---
phase: 20-transactional-delegated-mutation
plan: "19"
subsystem: sandbox
tags: [native-repair, appcontainer, windows, sandbox-boundary, hygiene-reset, construction-only]
requires: ["20-16"]
provides:
  - Hard hygiene reset of the tainted working tree to pristine be84bd2 (the boundary-breaking process.rs diagnostic discarded, not salvaged)
  - Fresh AppContainer is_available() write-access fix (REQ-native-r1)
  - Fresh drop-deny-only-SIDs sandbox read-boundary fix with isolation preserved (REQ-native-r2)
  - Fresh Windows security-constant import fix unblocking the msvc build (REQ-native-r3)
affects: ["20-20", "20-25"]
key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-19-SUMMARY.md
  modified:
    - crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
    - crates/wcore-agent/src/session_journal/snapshot.rs
key-decisions:
  - "Task 1 reset both spike/diagnostic files to committed HEAD (byte-identical to pristine be84bd2 for these paths); the reset produced zero delta so it carries no commit, and every fix in Task 2 was written fresh, never salvaged from the discarded diagnostic (REQ-native-r15)."
  - "The deny-only Administrators/Users/Authenticated-Users SID marking is redundant on a real AppContainer (containment is intrinsic to the package-SID access model) and was what blocked package-SID-granted reads; dropping it restores reads while the child token stays restricted, low-integrity, and AppContainer-tagged, and process creation still uses the restricted token — never the caller's full primary token."
  - "READ_CONTROL/WRITE_DAC are merged into the existing Win32::Storage::FileSystem import block (their real location in windows-sys 0.59) rather than left as a second module-path use statement."
requirements-completed: []
duration: ~1h
completed: 2026-07-23
status: complete
candidate_tuple:
  source_sha: 0e8e6c1d8ef3ce3780174a66aec0c5078fa0548d
  source_tree: 409fe8d1040512ed2a723fc2849ca0402a4fa075
  pristine_base_sha: be84bd2b9d8a340e85a27533286cc5d14dfae45d
  pristine_base_tree: 6d0948406d3d7835f7bd7d37397b77aa744484f4
---

# Phase 20 Plan 19: Native Sandbox Repair — Availability, Read-Boundary, and Windows Import

**The tainted working tree was reset to pristine `be84bd2`, then the three native defects the 2026-07-23 hardware investigation root-caused were written fresh as one clean candidate delta touching exactly the three declared source files — construction-proven on the Mac (fmt + scope) and Linux clippy-clean on Hetzner, with all Windows/macOS runtime proof deferred to the native-proof gate (20-25).**

## Candidate Tuple

- **source_sha:** `0e8e6c1d8ef3ce3780174a66aec0c5078fa0548d` — tree `409fe8d1040512ed2a723fc2849ca0402a4fa075`
- **pristine_base:** `be84bd2` — tree `6d09484` (last code commit; the intervening `0394963`..`3e48ad6` commits are planning-doc-only, so the three source paths at HEAD were byte-identical to `be84bd2` before any edit)
- **task_base (scope authority):** captured at `039496338eab40a0306da1d97e536fa604f29770`, generation `g-88f306a2…`

> Note on branch tip: after this fix commit landed, a separate docs-only HANDOFF commit (`4cd2263`, another agent on the shared branch) was fast-forwarded on top. The fix commit `0e8e6c1` is intact in history directly below it; the source delta of this plan is exactly `0e8e6c1`.

## Task 1 — Hard hygiene reset to pristine (REQ-native-r15)

The Mac working tree carried two uncommitted edits: a `.write(true)` spike in `storage.rs`, and a **boundary-breaking diagnostic** in `process.rs` (a `current_token` full-primary-token swap for `CreateProcessAsUserW`, `DISABLE_MAX_PRIVILEGE` neutered to `0`, and ~90 lines of env-gated `WCORE_DIAG_TOKEN` token-group dump). Both files were restored whole to committed HEAD with `git checkout --`.

Reset evidence:
- `git status --porcelain` for both files: empty (no working-tree modification).
- `grep -rn WCORE_DIAG_TOKEN crates/`: no matches — the diagnostic is gone.
- `git diff be84bd2 HEAD` for the two files: empty (byte-identical to the pristine baseline).

Because a reset to HEAD produces no delta, Task 1 carries no commit; it is a pure hygiene precondition. No fix was salvaged from the diagnostic — everything below was written fresh from the clean baseline.

## Task 2 — The three real fixes (written fresh)

**1. `acl_lease/storage.rs` `create_new_nofollow` — is_available() probe (REQ-native-r1).**
Added `.write(true)` to the `OpenOptions` chain (ahead of the explicit `.access_mode(GENERIC_READ | GENERIC_WRITE)`). std's `get_creation_mode` validates the high-level write/append flags independently of `access_mode`, so a `create_new` open with neither `.write(true)` nor `.append(true)` fails with `InvalidInput` before `CreateFileW` is called — the ACL-lease probe file is never created and `is_available()` returns false on every Windows host. `.write(true)` satisfies that gate; the effective access stays exactly `GENERIC_READ | GENERIC_WRITE`.

**2. `windows_impl/process.rs` `execute_blocking` — sandbox read boundary (REQ-native-r2).**
Changed the `CreateRestrictedToken` `SidsToDisable` arguments from `sids_to_disable.len() as u32, sids_to_disable.as_mut_ptr()` to `0, ptr::null_mut()`, and deleted the now-dead `admins_sid`/`users_sid`/`auth_users_sid` allocation and the `[SID_AND_ATTRIBUTES; 3]` array. The deny-only marking broke the AppContainer package-SID grant path (the child had no usable enabled SID for any file's DACL, so it could read no file at all). Isolation is intrinsic to the AppContainer access model — a file granted only to normal SIDs is still denied — so the deny-only marking was redundant. Preserved exactly as the pristine baseline: `DISABLE_MAX_PRIVILEGE`, the explicit low-integrity label, the AppContainer capability set, and restricted-token (never full-primary-token) process creation via `CreateProcessAsUserW(restricted_token.as_raw(), …)`. No full-token swap and no diagnostic were reintroduced.

**3. `session_journal/snapshot.rs` — Windows import (REQ-native-r3).**
Moved `READ_CONTROL`/`WRITE_DAC` out of `Win32::Security` and into the existing `Win32::Storage::FileSystem` import block, where windows-sys 0.59 actually exports them (the feature this crate already enables). Resolves `error[E0432]` on `x86_64-pc-windows-msvc`.

## Gate Results

### Mac construction (per-task, provisional)
- **Scope (`verify-task-scope.sh`):** `scope-ok base=0394963 … paths=3` — exactly the three declared paths, no out-of-scope drift.
- **Format (`vx rustfmt --edition 2024 --check`):** clean for all three files. (The plan's verify literal used `--edition 2021`, which rejects the pre-existing let-chains in `snapshot.rs`; the workspace edition is 2024, so the authoritative fmt is the Hetzner `cargo fmt` below, which uses the workspace edition and passes.)
- **`snapshot.rs` import grep (`Win32::Storage::FileSystem::{`):** present.

### Hetzner Linux (committed-HEAD authoritative)
Proven at build-clone HEAD `0e8e6c1` (fmt/clippy) and `4cd22631` (tests; same source blobs for the three files):
- **`vx cargo fmt --all --check`:** clean (no diff).
- **`vx cargo clippy -p wcore-sandbox -p wcore-agent --all-targets --all-features -- -D warnings`:** **EXIT=0**. The only note is a pre-existing `imap-proto v0.10.2` future-incompat in a transitive dependency — not a clippy warning and not in scope.
- **`vx cargo nextest run -p wcore-sandbox -p wcore-agent`:** 2982/2982 passing with the one known-slow test excluded (`-E 'not test(fix1_dispatch_budget_aborts_with_partial_result)'`).

### Deferred issue (out of scope — pre-existing, unrelated)
`wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` times out at the nextest 60s per-test cap **deterministically** on the Hetzner box — it also timed out in isolation and at both `0e8e6c1` and `4cd22631`. This plan's changes are entirely `#[cfg(windows)]` (they do not compile on Linux at all), so they cannot affect this Linux dispatch-budget timing test. It is a pre-existing environment/timing issue in the test harness, unrelated to 20-19, and is logged here rather than fixed (scope boundary).

## Linux-proven vs. deferred-to-20-25

- **Linux-proven here:** no Linux regression — `wcore-sandbox` + `wcore-agent` clippy-clean at `-D warnings`, fmt-clean, and (excluding the one pre-existing timeout) full green. Diff hygiene and scope are enforced.
- **UNPROVEN here, deferred to the native-proof gate (20-25):** every Windows and macOS runtime claim — `is_available()` returning true on a real AppContainer host, the read/grant/revoke acceptance path going green, and the `x86_64-pc-windows-msvc` build. The three touched code regions are all `cfg(windows)`, so Linux clippy compiles the surrounding crate but not the changed branches; Windows compilation and behavior are proven only on the msvc self-hosted runner at 20-25.

## Requirements & Claims

No Phase 20 requirement is marked complete by this plan. REQ-native-r1/r2/r3 exist as reviewable, gated source; REQ-native-r15 (hygiene reset) is satisfied. No aggregate, native, requirement, or phase claim is made. The candidate proceeds to the later native-proof and review gates.

## Self-Check: PASSED

- `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs` — FOUND (modified; `.write(true)` present)
- `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs` — FOUND (modified; `0, ptr::null_mut()` SidsToDisable; deny-only block removed)
- `crates/wcore-agent/src/session_journal/snapshot.rs` — FOUND (modified; `READ_CONTROL`/`WRITE_DAC` under `Win32::Storage::FileSystem`)
- Fix commit `0e8e6c1` — FOUND in `git log` (history-verified, intact below the docs HANDOFF commit)

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-23*
