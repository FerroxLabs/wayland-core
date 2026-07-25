---
phase: 20-transactional-delegated-mutation
plan: "36"
subsystem: testing
tags: [native-uat, f20, windows-msvc, appcontainer, job-object, macos-ephemeral, docker, exit-code, sealed-repaired-successor]
disposition: CONSTRUCTION-COMPLETE / NATIVE-DEFERRED

# Dependency graph
requires:
  - phase: 20-32
    provides: "Retained candidate-bound native RED evidence for the sealed successor 17412cf2 (both platforms RED on NEW distinct causes: Windows live_fs_acl exit-0 defect at :382; macOS missing alpine:3.19 image)"
  - phase: 20-30
    provides: "Sealed candidate 17412cf2 (tree 00e41519) — Linux 11509/0 + all-features build"
  - phase: 20-20
    provides: "The choice-hold + (now-superseded) `& ver >nul` exit-0 tail whose ERRORLEVEL assumption this plan corrects"
provides:
  - "New repaired-successor candidate daf27337 (tree 91de96a3) fixing exactly the two NEW 20-32 RED causes and nothing else"
  - "Windows granted-read/parent-exit exit-0 fidelity via `exit /b 0` (deterministically sets the exit code; `ver` does not reset ERRORLEVEL) at every sibling site"
  - "macOS proof harness self-provisions its docker test image (alpine:3.19) fail-closed before the docker targets"
affects: [20-37, 20-38, 20-39, 20-40, 20-41, 20-42]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "cmd.exe exit-0 normalization must SET ERRORLEVEL (`exit /b 0`), never merely print (`ver`), after a `choice`-based hold that leaves a residual 1-based selection index"
    - "Ephemeral-runner docker targets pre-pull their image set (duplicate-with-pointer to the Rust source of truth) fail-closed before any target runs"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-36-SUMMARY.md
  modified:
    - crates/wcore-sandbox/tests/live_fs_acl.rs
    - crates/wcore-sandbox/tests/hard_process_containment_windows.rs
    - scripts/f20-native-macos-proof.sh

key-decisions:
  - "Fixed the exit-0 tail with `exit /b 0` (the plan's primary recommendation), confirmed from cmd.exe ERRORLEVEL semantics: `exit /b N` SETS the process exit code to N and returns; in a `cmd /c` string (no batch file) it terminates cmd with N. This is the root-cause fix for the residual `choice` index surviving as the cmd /c exit code — unlike `ver`, which prints the OS version and leaves ERRORLEVEL untouched."
  - "Applied the SAME `exit /b 0` primitive at EVERY sibling `& ver >nul` parent-exit site so the native proof cannot peel to the next onion layer one target later."
  - "Encoded the macOS docker image set as a shell list (alpine:3.19) with pointer comments to BOTH docker_smoke.rs AND workspace_authority.rs (mirroring the SANDBOX_ACTIVE_PROCESS_LIMIT duplicate-with-pointer convention), never by parsing Rust."
  - "Left `contained_detached_child_exit` (`& exit {code}`) and the bare-`ver` preflight probe (`cmd /c \"ver >nul\"`) unchanged — both are audit-confirmed deterministic/fresh-context and not defects."

patterns-established:
  - "Exit-code fidelity: after a `choice` hold, use `exit /b 0` (sets ERRORLEVEL), not `ver >nul` (prints only)."

requirements-completed: []  # Per plan: complete NO Phase 20 requirement. That is 20-42.
---

# Phase 20 Plan 36: Second Repaired-Successor — Windows exit-0 fidelity + macOS docker-image provisioning — Summary

**One-liner:** Repaired the two exact NEW 20-32 RED causes on a clean successor of the sealed candidate `17412cf2` — (1) replaced the `& ver >nul` exit-0 tail with a deterministic `exit /b 0` at every Windows granted-read and Job-Object parent-exit site (`ver` prints the version but does NOT reset ERRORLEVEL, so `choice.exe`'s residual 1-based selection index survived as the `cmd /c` exit code and RED'd `live_fs_acl.rs:382` on real hardware), and (2) added a fail-closed `docker pull alpine:3.19` to the macOS proof harness after the `docker info` gate and before any docker target. The delta touches exactly the three declared files; fmt-clean, scope-clean, the sanctioned macOS `cargo check` green, and Hetzner clippy `-D warnings` + `nextest -p wcore-sandbox` show no Linux regression. NO native, aggregate, or requirement claim is made.

## Disposition

**CONSTRUCTION-COMPLETE / NATIVE-DEFERRED.** The new candidate is sealed-ready for 20-37. All Windows compile + real-hardware exit-code behavior and macOS docker-target green are deferred to the 20-39 native-proof re-dispatch (fresh Sean authorization required). No Phase-20 requirement is completed.

## New candidate (successor of the sealed 17412cf2)

- `source_sha = daf273373eddcb22a94e26988747e6f74ff81bde`
- `source_tree = 91de96a3f703423dac23408f20b397b6fbfeee00`
- Successor of sealed candidate `17412cf2f6a8be9d2ec7272f6693f998db4ba2e5` (tree `00e41519ac6782b05e610fcf7fafc772d5040a5d`).
- **No source drift confirmed before editing:** the three touched files' pre-edit blobs equalled the sealed tree blobs (`live_fs_acl.rs=d196b45c`, `hard_process_containment_windows.rs=2dd35a03`, `f20-native-macos-proof.sh=3f7666ff`).
- **Delta vs sealed is exactly the three source files** (`git diff --name-only 17412cf2 HEAD` shows only those three plus planning docs 20-29…20-42, which are the inter-seal planning-doc commits the plan anticipated). The candidate adds ONLY the two intended fixes and nothing else.

## Task commits

| Task | Commit | Files | Gate |
|------|--------|-------|------|
| 1 — Windows exit-0 fidelity | `8b8cf5bf30bdb69545d6996886e2e0dbfbb88aaa` | `live_fs_acl.rs`, `hard_process_containment_windows.rs` | scope=2, fmt-clean |
| 2 — macOS docker pre-pull | `daf273373eddcb22a94e26988747e6f74ff81bde` | `f20-native-macos-proof.sh` | scope=1, `bash -n` OK |

Per-task disjoint scope enforced via `verify-task-scope.sh` (base captured at `a9f94615`; advanced `--start-fresh` to Task 1 tip `8b8cf5bf` / generation `g-7e1c68c6…` before Task 2).

## The fix — Task 1 (Windows exit-0 fidelity)

**Root cause (confirmed against source + the 20-32 RED, not restated blindly):** the success/parent-exit paths ended with `& ver >nul` to force exit 0. `choice.exe /T N /D Y` sets ERRORLEVEL to its 1-based selection index (default `Y` => 1, never 0). `ver` PRINTS the OS version and does NOT modify ERRORLEVEL; `cmd /d /s /c` returns ERRORLEVEL as it stands at exit, so the residual `choice` index (1) survived as the process exit code on real Windows hardware — the 20-20 assumption that `ver` "always exits 0" is false for exit-code purposes. On Linux this was never observed because both files are `#![cfg(windows)]` (zero compiled tests).

**Fix:** replaced each `& ver >nul` tail with `& exit /b 0` — a primitive that deterministically SETS the process exit code to 0. Preserved at every site: (a) the `type "<file>" && ( … )` denied-read short-circuit; (b) the stdin-free `choice` hold; (c) `format!` placeholder/argument counts.

### Windows sibling / 6-target audit ledger

| Site (function) | File | Selected by native target(s) | Disposition |
|-----------------|------|------------------------------|-------------|
| `type_and_hold` | live_fs_acl.rs | windows-retained-handle (`one_execution_grant_never_leaks_to_another_identity`, the observed RED at :382) + windows-appcontainer-acl (`granted_path_is_readable_then_revoked`) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `echo_temp_and_hold` | live_fs_acl.rs | (consumed by `twenty_concurrent_executions_have_unique_temp_roots`, `timeout_and_cancellation_remove_their_leases`) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `job_close_reaps_detached_descendant_with_no_residue` | hard_process_containment_windows.rs | (Job-Object containment) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `active_process_cap_is_enforced` | hard_process_containment_windows.rs | (Job-Object containment) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `breakaway_is_denied` | hard_process_containment_windows.rs | (Job-Object containment) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `qualified_hard_containment_backend_preflight` (2nd exec) | hard_process_containment_windows.rs | (Job-Object containment preflight) | **FIXED** `& ver >nul` → `& exit /b 0` |
| `contained_detached_child_exit` | hard_process_containment_windows.rs | (exit-code fidelity) | **UNCHANGED (audit-confirmed correct)** — already `& exit {code}` (deterministic, codes 0 and 7) |
| `qualified_hard_containment_backend_preflight` (1st exec) | hard_process_containment_windows.rs | (benign preflight probe) | **UNCHANGED (audit-confirmed non-defect)** — bare `cmd /c "ver >nul"`: a fresh cmd starts at ERRORLEVEL 0 with no preceding `choice`; `ver` leaves it 0. Not a defect; left as-is for minimal scope. |
| `windows-public-dispatch` (`dispatch_smoke`) | dispatch_smoke.rs | 'any' target, Linux-covered | **NO CHANGE (audit-recorded)** — no `choice`/`ver`/exit-code batch tail; the Windows-specific behavior is a host-side `std::fs::rename` (line 333), not a sandboxed exit-code script, so the `ver` exit-code defect class does not apply. |
| `windows-f20-lifecycle` (`transactional_delegated_mutation_test`) | transactional_delegated_mutation_test.rs | 'any' target, Linux-covered | **NO CHANGE (audit-recorded)** — no `choice`/`ver`/exit-code batch tail, no cfg(windows) batch scripts. |

**Also updated (in-scope, same lines):** the now-stale doc comments/assertion messages that described the old `ver` mechanism (`type_and_hold`/`echo_temp_and_hold` doc comments, the `// exit 0 via type && … & ver` comment in `one_execution_grant_never_leaks_to_another_identity`, the `(ver => 0)` parenthetical in two `assert_eq!` messages, and the `then exit 0 (ver)` fan-out comment) were rewritten to describe `exit /b 0` accurately. No assertion, `choice` hold, or denied-read gate was weakened or removed.

## The fix — Task 2 (macOS docker-image provisioning)

`docker_smoke.rs` hardcodes `image: "alpine:3.19"` in its three daemon-touching tests; `docker_runs_hello_world` (`.await.unwrap()`) HARD-fails on a missing image, which RED'd `macos-docker-roundtrip-delete` at 20-32 on the fresh ephemeral runner. Added a docker-image pre-pull step to `f20-native-macos-proof.sh` **after** the `docker info` liveness gate (line 109) and **before** the first `run_target` (line 288): a `DOCKER_TEST_IMAGES=("alpine:3.19")` loop that `docker pull`s each image and fails closed (`exit 1` with a clear message) on any pull failure under `set -euo pipefail`.

**Image set derivation (plan-checker WARNING folded):** derived from ALL docker-touching macOS targets, not just `docker_smoke.rs` — that's `docker_smoke.rs` (alpine:3.19, targets macos-docker-reject-path-replacement / -roundtrip-delete / -cancellation) AND `crates/wcore-swarm/tests/workspace_authority.rs` (alpine:3.19 at :114, target macos-docker-budget). Both hardcode the same single image today; the list carries pointer comments to BOTH source files so a future image divergence is caught. Encoded as a duplicate-with-pointer shell list, not by fragile-parsing Rust.

**Unchanged:** the exact-checkout/repo-root/exact-commit/exact-tree gates, the env/Darwin/`docker info` gates, the wrong-OS anti-drift guard, the eight `run_target` selectors and their order, and the ordered per-target + single final acceptance marker grammar (`run_target` count = 8; `F20_NATIVE_MACOS_ACCEPTANCE=PASS` count = 1, both verified post-edit).

## Verification receipts

Construction gates (per task, on the Mac):
- **Task 1 scope:** `scope-ok base=a9f94615 generation=g-2993b222… paths=2`
- **Task 1 fmt:** `vx rustfmt --edition 2024 --check` — FMT-CLEAN (both files)
- **Task 2 scope:** `scope-ok base=8b8cf5bf generation=g-7e1c68c6… paths=1`
- **Task 2 shell syntax:** `bash -n scripts/f20-native-macos-proof.sh` — SYNTAX-OK

Post-both-commit gates (clean checkout):
- **Sanctioned macOS compile (D5 carve-out) — `cargo check -p wcore-sandbox --features live-docker --tests`:** GREEN (`Finished dev profile … in 0.91s`). Re-confirms the macOS-compiled surface (lib + docker_smoke + macOS-gated tests) still compiles. It does NOT compile the edited `#![cfg(windows)]` helpers. One PRE-EXISTING, out-of-scope warning surfaced: `method \`signal\` is never used` at `crates/wcore-sandbox/src/backends/process_tree.rs:403` (macOS-gated production source I did not touch; present on the sealed candidate; the macOS gate is `cargo check`, not `-D warnings`, so it does not fail).
- **Hetzner clippy (committed-HEAD, slot f20-36-clippy) — `clippy -p wcore-sandbox --all-targets --all-features -- -D warnings`:** GREEN (`Finished dev profile … in 10.98s`, zero warnings under `-D warnings`).
- **Hetzner nextest (committed-HEAD, slot f20-36-nextest) — `nextest run -p wcore-sandbox`:** GREEN — **100 tests run: 100 passed, 2 skipped, 0 failed.** The edited `#![cfg(windows)]`/bash surface compiles to nothing on Linux, so the Linux test surface is unchanged. (The prompt's noted `fix1_dispatch_budget_aborts_with_partial_result` default-profile flake is a `wcore-agent` test — not in this `wcore-sandbox` run.)

## Re-dispatch structure (per plan `<redispatch_decision>`)

New additive serial chain 20-36 (fix) → 20-37 (re-seal) → 20-38 (pre-native cross-audit) → 20-39 (native re-dispatch) → 20-40 (fresh review) → 20-41 (re-prep tuple) → 20-42 (terminal). NOT a re-bind: 20-32 stays as retained RED evidence (its `refs/f20-native-uat/17412cf2…` ref is retained; deletion is a Sean-only gate); 20-33/20-34/20-35 are superseded (dead — their upstream 20-32 is terminal RED) and are neither executed nor edited by this chain.

## Explicit non-claims / deferrals

- **Windows-exit-code-proof-deferred-to-20-39.** The Windows fix lives entirely in `#![cfg(windows)]` files; it compiles to NOTHING on this Mac or on Hetzner Linux. NO pre-dispatch Windows compile or behavior proof is claimed. Its compile + real-hardware exit-code behavior are proven ONLY on the self-hosted msvc AppContainer runner at the 20-39 re-dispatch.
- **macOS docker-target green deferred to 20-39.** The pre-pull is a candidate change; that all 8 macOS targets now run green on the ephemeral runner is proven at 20-39, not here.
- **No native, aggregate (11509/0/48), or Phase-20 requirement claim is made.** Requirements are completed at 20-42.

## Self-Check: PASSED

- `crates/wcore-sandbox/tests/live_fs_acl.rs` — FOUND (modified, committed `8b8cf5bf`)
- `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` — FOUND (modified, committed `8b8cf5bf`)
- `scripts/f20-native-macos-proof.sh` — FOUND (modified, committed `daf27337`)
- Commit `8b8cf5bf` — FOUND in `git log`
- Commit `daf27337` — FOUND in `git log` (HEAD)
- `.planning/phases/20-transactional-delegated-mutation/20-36-SUMMARY.md` — FOUND (this file)
