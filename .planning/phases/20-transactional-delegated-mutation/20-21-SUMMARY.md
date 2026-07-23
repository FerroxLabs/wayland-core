---
phase: 20-transactional-delegated-mutation
plan: "21"
subsystem: testing
tags: [native-repair, appcontainer, windows, job-object, proof-harness, anti-drift, construction-only]

# Dependency graph
requires:
  - phase: "20-20"
    provides: Windows exit-code + directory-rename test-bug fixes and AppContainer isolation proofs; candidate base 31844fb1
provides:
  - Real Windows Job-Object hard-containment acceptance tests (exit-code fidelity, KILL_ON_JOB_CLOSE no-residue, active-process cap, breakaway denial, hard-containment preflight) (REQ-native-r7)
  - Two Windows containment proof targets repointed off the Linux-only Bubblewrap test onto the new Job-Object tests (REQ-native-r7)
  - Wrong-OS anti-drift guard in the Windows proof harness that fails closed when a native target maps to a wrong-OS test (REQ-native-r8)
  - Shared canonical native-target -> {crate,test,os} expectation map (WINDOWS_TARGET_SOURCES / MACOS_TARGET_SOURCES) the macOS guard reuses in 20-22
affects: ["20-22", "20-25", "20-16-review"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Black-box Windows Job-Object containment proof through the public SandboxBackend surface only: detached descendants observed alive mid-flight then asserted reaped-with-no-residue via a host-side CIM/tasklist liveness query (no production test-seam)"
    - "Self-match-free host process query: CIM `-Filter \"Name='cmd.exe'\"` excludes the querying powershell.exe even though its own command line carries the tag"
    - "Declarative per-target expected-OS field + fail-closed positive/negative cfg-gate guard: an OS-specific proof target's selected test source must be affirmatively cfg-gated for its OS and not for a foreign OS; cross-platform targets are marked 'any' and exempt"

key-files:
  created:
    - crates/wcore-sandbox/tests/hard_process_containment_windows.rs
    - .planning/phases/20-transactional-delegated-mutation/20-21-SUMMARY.md
  modified:
    - scripts/f20-native-windows-proof.ps1
    - scripts/f20-native-uat-proof.mjs

key-decisions:
  - "Authored the Windows containment tests entirely through the wcore-sandbox PUBLIC surface (AppContainerBackend::execute + SandboxManifest/SandboxCommand + SandboxBackend trait) rather than exposing a test-only seam, because files_modified declares exactly 3 paths and a production accessor would be out of scope. All five properties are provable black-box."
  - "The anti-drift guard uses a per-target `os` field ('windows'|'macos'|'any'). OS-specific targets must have their selected test source affirmatively cfg-gated for that OS (positive gate) and not gated for a foreign OS (negative gate); 'any' targets (dispatch_smoke, f20 lifecycle — cross-platform, Linux-green) are exempt. The positive gate is load-bearing: the Bubblewrap test carries NO windows cfg, so a Windows target re-pointed at it fails closed."
  - "windows-job-object selects all four Job-Object mechanism tests (exit fidelity, no-residue, active-process cap, breakaway); windows-hard-process-containment selects the preflight — so every authored test is covered by a target under --no-tests=fail."
  - "The mjs meaningful change is the shared canonical WINDOWS_TARGET_SOURCES / MACOS_TARGET_SOURCES expectation the ps1 guard mirrors and the macOS guard reuses in 20-22; WINDOWS_TARGETS ids/order and the marker grammar are unchanged so the existing node --test suite stays green."

requirements-completed: []

# Coverage metadata
coverage:
  - id: R7
    description: "Real Windows Job-Object containment tests exist and the two containment targets select them"
    requirement: "REQ-native-r7"
    verification:
      - kind: integration
        ref: "crates/wcore-sandbox/tests/hard_process_containment_windows.rs (5 native #[cfg(windows)]/#[ignore] cases)"
        status: unknown
    human_judgment: true
    rationale: "Native #[cfg(windows)] Job-Object tests; cannot compile or run on Linux/macOS. Real green is proven only on the self-hosted msvc AppContainer host at the 20-25 native-proof gate. Target selection (ps1) is Linux-verifiable and verified."
  - id: R8
    description: "A structural guard makes a native proof target unable to silently map to a wrong-OS test (fails closed)"
    requirement: "REQ-native-r8"
    verification:
      - kind: harness
        ref: "scripts/f20-native-windows-proof.ps1 Assert-TargetOsGate (positive+negative cfg-gate check per OS-specific target)"
        status: pass
    human_judgment: true
    rationale: "The guard's premises are Linux-verified: the two windows-gated targets' sources carry #![cfg(windows)]; the Bubblewrap test carries zero windows cfg, so a re-point fails the positive gate closed. The PowerShell guard itself executes on the Windows runner (pwsh is not installed on Hetzner/Mac by decision), so its runtime pass is observed at 20-25."

# Metrics
duration: ~50min
completed: 2026-07-23
status: complete
---

# Phase 20 Plan 21: Windows Job-Object Containment Tests + Harness Repoint + Anti-Drift Guard Summary

**Authored the real Windows Job-Object hard-containment acceptance tests where none existed (the two "Windows" containment targets were wired to the Linux-only Bubblewrap test), repointed both proof targets onto them, and added a wrong-OS anti-drift guard that fails closed when a native target maps to a wrong-OS test — construction-proven on the Mac and Linux-clean (no regression) on Hetzner; all Windows containment green explicitly deferred to the 20-25 native-proof gate.**

## Candidate Tuple

- **source_sha:** `9cf5666ebfd02808b1bd0f8d7abf284ff82126c8` — tree `93cf8c21401842f6e74d37c6504acfeb2a540a1b`
- **inherited base (20-20):** `source_sha=31844fb1`, tree `f2fe177a`; pristine base `be84bd2`
- **task_base (scope authority):** captured at `772b2b93cc70d221573bcee7f5f9809ec4669d36`, tree `56a4bb44`, generation `g-6a5bd8df…`
- **Touched paths (scope-verified, exactly 3):**
  - `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (new)
  - `scripts/f20-native-windows-proof.ps1`
  - `scripts/f20-native-uat-proof.mjs`
  - No production source changed.

## Performance

- **Duration:** ~50 min
- **Completed:** 2026-07-23
- **Tasks:** 2
- **Files modified/created:** 3 (+ this summary)

## Accomplishments

- **REQ-native-r7 (real Windows Job-Object tests + repoint):** authored `hard_process_containment_windows.rs`, five `#[cfg(windows)]`/`#[ignore]` native acceptance cases that drive the ACTUAL Job Object the AppContainer backend installs in `windows_impl/process.rs`, through the `wcore-sandbox` public surface only:
  - `contained_detached_child_exit` — exit-code fidelity on BOTH the zero and non-zero terminal paths through the Job-Object-wrapped execution, plus a falsifiable descendant-reaping wall-clock bound (a leaked detached idler holding the pipe would block execute past the 20s bound).
  - `job_close_reaps_detached_descendant_with_no_residue` — KILL_ON_JOB_CLOSE proven by an explicit host-side liveness query: a tagged detached grandchild is observed RUNNING mid-flight, then asserted GONE after job close (no residue), not merely inferred from parent exit.
  - `active_process_cap_is_enforced` — a fan-out beyond the Job Object's `ActiveProcessLimit` is bounded: the observed concurrent descendant peak stays at/below the cap and below the attempted count (excess spawns rejected).
  - `breakaway_is_denied` — with `BREAKAWAY_OK`/`SILENT_BREAKAWAY_OK` cleared, detached children cannot escape the job and are reaped on close.
  - `qualified_hard_containment_backend_preflight` — the backend self-reports hard descendant containment (`owns_descendants_hard`/`enforces_read_deny`/`blocks_powershell`) and a benign contained command runs end to end plus a detached-descendant reap smoke.
  Both `windows-job-object` and `windows-hard-process-containment` targets in `f20-native-windows-proof.ps1` now select these tests (never the Linux-only `hard_process_containment.rs`).
- **REQ-native-r8 (wrong-OS anti-drift guard):** each native proof target now declares an expected `os`; `Assert-TargetOsGate` runs before cargo and fails closed unless an OS-specific target's selected test source is affirmatively cfg-gated for its own OS (positive gate) and carries no foreign-OS cfg (negative gate). The positive gate is the load-bearing catch — the Bubblewrap test has zero windows cfg, so re-pointing a Windows target back at it fails closed. Cross-platform (`os = 'any'`) targets (`dispatch_smoke`, the f20 lifecycle) are exempt.
- **Shared expectation for the macOS side (20-22):** `f20-native-uat-proof.mjs` now exports `WINDOWS_TARGET_SOURCES` / `MACOS_TARGET_SOURCES` — the canonical id → `{crate,test,os}` map the ps1 guard mirrors and the macOS guard reuses when its application is completed in 20-22. `WINDOWS_TARGETS` ids/order and the marker grammar are unchanged.

## Task Commits

1. **Task 1: Author the real Windows Job-Object hard-containment tests** — `b533c48c` (test)
2. **Task 2: Repoint the Windows proof targets and add the wrong-OS anti-drift guard** — `9cf5666e` (test/harness)

## Gate Results

### Mac construction (allowed operations only)
- **Scope (`verify-task-scope.sh`):** Task 1 `scope-ok … paths=1`; Task 2 `scope-ok base=772b2b93 generation=g-6a5bd8df… paths=3` — exactly the three declared paths, no drift.
- **rustfmt (`vx rustfmt --edition 2024 --check`):** clean for the new test file. (The plan's verify literal hardcodes `--edition 2021`; the workspace is edition 2024, so 2024 is the authoritative fmt — confirmed by the Hetzner `cargo fmt --all --check` below and consistent with the 20-19/20-20 edition note.)
- **Name grep:** all five named tests present in the new file.
- **`node --check` + `node --test scripts/f20-native-uat-proof.test.mjs`:** syntax OK; UAT-proof self-test suite **34 passed / 0 failed**.
- **Target-map grep:** ps1 references `hard_process_containment_windows` and no longer selects the `hard_process_containment` (Bubblewrap) test.

### Hetzner Linux (committed-HEAD authoritative, build-clone HEAD `9cf5666e`)
- **`vx cargo fmt --all --check`:** clean (EXIT=0).
- **`vx cargo clippy -p wcore-sandbox --all-targets --all-features -- -D warnings`:** **EXIT=0**.
- **`vx cargo nextest run -p wcore-sandbox`:** **100 passed, 2 skipped, 0 failed (EXIT=0)**. The 2 skips are the pre-existing `#[ignore]` native/live cases. The new `hard_process_containment_windows.rs` is `#![cfg(windows)]`, so on Linux it compiles to nothing and contributes zero tests — exactly as designed.

Note: the known-slow `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` timeout is a pre-existing Linux flake at every SHA and is out of scope here — this plan changes `wcore-sandbox` (a `#[cfg(windows)]` test) + two non-Rust scripts, so that `wcore-agent` test never runs. Noted, not fixed, not a blocker.

## Linux-proven vs. deferred-to-20-25

- **Linux-proven here (no regression):** `wcore-sandbox` is fmt-clean, clippy-clean at `-D warnings`, and full-green on nextest at the candidate HEAD. The two non-Rust scripts change no compiled surface. The anti-drift guard's *premises* are Linux-verified: the two windows-gated targets' sources carry `#![cfg(windows)]`, and the Bubblewrap test carries zero windows cfg (so a re-point fails the positive gate closed). The UAT-proof node self-tests stay green. Scope + diff hygiene enforced (exactly 3 declared paths).
- **UNPROVEN here, explicitly deferred to the native-proof gate (20-25):** every Windows RUNTIME claim. `hard_process_containment_windows.rs` is `#![cfg(windows)]`, so Linux compiles nothing in it — the five Job-Object containment properties (exit-code fidelity, KILL_ON_JOB_CLOSE no-residue, active-process cap, breakaway denial, hard-containment preflight) are proven ONLY on the self-hosted msvc AppContainer runner at 20-25. The PowerShell `Assert-TargetOsGate` guard's runtime execution is likewise observed at 20-25 (pwsh is not installed on Hetzner/Mac by CONTEXT decision). **No native claim is made from Linux or from source inspection.**

## Explicit non-claims

- No Windows Job-Object containment green is claimed from this Linux run.
- No Phase 20 requirement is marked complete. REQ-native-r7/r8 exist as reviewable, gated, construction-proven artifacts plus a Linux no-regression result.
- No aggregate / native / phase claim is made.

## Deviations from Plan

- **[Scope-driven] Windows tests authored via the public surface, no test-only seam.** The plan's Task 1 action permits exposing a minimum test-only production accessor "if the Job-Object handle or preflight is not reachable from a test-visible seam." It was reachable enough: all five containment properties are provable black-box through `AppContainerBackend::execute` + the `SandboxBackend` trait, and `files_modified` declares exactly the 3 non-production paths. Adding a production accessor would have violated scope, so none was added. Consequence: `active_process_cap_is_enforced` observes the cap by a real bounded fan-out (host-side process count) rather than reading the internal `SANDBOX_ACTIVE_PROCESS_LIMIT` (it is `pub(super)`); the cap value 512 is duplicated in the test with a pointer to `windows_impl/command.rs` as its source of truth. **20-25 note:** the fan-out count/timing and the execute-permission assumptions of the containment scripts should be confirmed (and may be lightly tuned) on the real AppContainer host.

## Self-Check: PASSED

- `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` — FOUND (5 named tests + zero-execution guard; `#![cfg(windows)]`)
- `scripts/f20-native-windows-proof.ps1` — FOUND (two containment targets repointed to `hard_process_containment_windows`; `Assert-TargetOsGate` added and wired into the target loop; per-target `os` fields)
- `scripts/f20-native-uat-proof.mjs` — FOUND (`WINDOWS_TARGET_SOURCES` / `MACOS_TARGET_SOURCES` added; `WINDOWS_TARGETS` ids/order unchanged)
- Task 1 commit `b533c48c` — FOUND in `git log`
- Task 2 commit `9cf5666e` — FOUND in `git log`
- Hetzner Linux at HEAD `9cf5666e`: fmt EXIT=0, clippy `-D warnings` EXIT=0, nextest 100 passed / 0 failed; node --test 34/0.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-23*
