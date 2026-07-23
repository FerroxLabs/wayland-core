---
phase: 20-transactional-delegated-mutation
plan: "22"
subsystem: testing
tags: [native-repair, macos, appcontainer, msvc, anti-drift, proof-harness, writer-verifier-parity, review-profile, construction-only]

# Dependency graph
requires:
  - phase: "20-21"
    provides: Windows Job-Object containment tests + wrong-OS anti-drift guard (ps1) + shared WINDOWS_TARGET_SOURCES / MACOS_TARGET_SOURCES map; candidate base 9cf5666e / prior HEAD c2b26c02
provides:
  - Re-validated macOS proof harness with the wrong-OS anti-drift guard applied on the macOS side (REQ-native-r9)
  - Windows candidate native leg pinned to the AppContainer-capable self-hosted msvc runner labels (REQ-native-r11)
  - Writer/verifier parity in f20-native-uat-proof.mjs — the sole durable request writer persists exactly the pending tuple the verifier later reads (STATE writer-gap closed)
  - Native-inclusive f20-native-16 review profile (native NOT deferred) + f20-native-crossaudit profile for the pre-native gate (REQ-native-r13)
affects: ["20-25", "20-26", "20-27", "20-28"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Filename-independent macOS anti-drift guard: resolves a target's source by the file that DEFINES the selected test (the --test binary, or the single tests/*.rs declaring the test(<fn>) function), then gates on cfg ATTRIBUTES only (never doc-comment prose) for a positive macOS gate + foreign-OS negative gate; 'any' targets exempt"
    - "Durable no-follow mode-0600 fsync'd request writer symmetric to the no-follow exact-byte reader; canonical fixed-field-order single-line JSON makes exact-tuple re-request byte-identical idempotent; conflicting/malformed/non-pending existing objects fail closed"
    - "Library module with a direct-execution-only CLI guard (realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) so importing stays side-effect-free while `node <mod> request` persists"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-22-SUMMARY.md
  modified:
    - scripts/f20-native-macos-proof.sh
    - .github/workflows/nightly-windows-soak.yml
    - scripts/f20-native-uat-proof.mjs
    - .planning/scripts/verify-review-result.mjs

key-decisions:
  - "macos-retained-directory now selects its macOS test by FUNCTION NAME (`-E test(required_live_macos_retained_directory_confines_writes)`), dropping the `--test live_integrity_macos` binary selector. Reason: the plan's Task-1 verify literal `! grep 'live_integrity'` (guarding against the 07-22 Windows-only `live_integrity.rs` mis-map) is a substring match that also catches the legitimate repaired macOS file `live_integrity_macos.rs` (created by prior plan 3383a46d). Renaming that Rust test is out of this plan's 4-file scope, so the honest in-scope resolution is to select by the macOS-specific function name (no `live_integrity` substring) and make the guard resolve source filename-independently. This is a stronger form of the 20-21 guard, not evasion."
  - "The macOS anti-drift guard gates on cfg ATTRIBUTE lines (`#[cfg...]`/`#![cfg...]`) only. The macОS test files legitimately mention `#![cfg(windows)]` in doc comments when documenting the cross-platform mirror; a naive whole-file grep tripped the negative gate on that prose, so gating is restricted to real attributes."
  - "The Windows candidate `runs-on` uses the self-hosted label list `[self-hosted, Windows, X64, msvc]` (mirrors the macOS job's list form). Hosted windows-2022 reports AppContainerBackend::is_available()==false, so the hard-containment proof can never run there. Scheduled soak, macOS ephemeral pin, contents:read-only, no-secrets, and no-cache posture unchanged; runner identity is captured via the run's runner_id/runner_name (bindRun), so no extra step was added."
  - "The request writer is added as the SOLE persister: persistRequest(path, requested) reads any existing pending object with the no-follow reader, reconciles via the existing reconcileRequest (idempotent / fail-closed), and writes canonical bytes via a new no-follow mode-0600 fsync'd writer. A `request` subcommand resolves a Git-private path via `git rev-parse --git-path` and runs only on direct execution. The existing 34-case suite and verifier primitives are untouched."

requirements-completed: []

# Coverage metadata
coverage:
  - id: R9
    description: "macOS proof harness re-validated; all 8 targets map to existing macOS-gated tests; wrong-OS anti-drift guard applied and fails closed"
    requirement: "REQ-native-r9"
    verification:
      - kind: harness
        ref: "scripts/f20-native-macos-proof.sh assert_target_os_gate (positive macOS cfg-attr gate + foreign-OS negative gate; source resolved by defining file)"
        status: pass
      - kind: integration
        ref: "8 macOS targets resolve to existing tests: live_integrity_macos.rs + hard_process_containment_macos.rs (#![cfg(target_os=macos)]), docker_smoke.rs, workspace_authority.rs, dispatch.rs lib, transactional_delegated_mutation_test.rs"
        status: unknown
    human_judgment: true
    rationale: "The guard's premises + fail-closed behavior are Mac-verified (8/8 construction cases: real macos targets pass, any exempt, every wrong-OS/missing/no-crate/unknown-os mapping fails closed). macOS RUNTIME green (cargo/docker) is proven only on the Scaleway Apple-silicon runner at 20-25."
  - id: R11
    description: "Windows candidate native leg pinned to the self-hosted msvc AppContainer-capable runner"
    requirement: "REQ-native-r11"
    verification:
      - kind: harness
        ref: ".github/workflows/nightly-windows-soak.yml f20-windows-candidate runs-on == [self-hosted, Windows, X64, msvc]; YAML parses; scheduled soak + macOS leg intact"
        status: pass
    human_judgment: true
    rationale: "YAML parse + assertions confirm the pin and the unchanged surrounding posture. That AppContainer is actually present on the selected runner is observed only when the leg runs at 20-25."
  - id: R13
    description: "Native-inclusive review profile (native NOT deferred) + cross-audit profile exist and schema-validate"
    requirement: "REQ-native-r13"
    verification:
      - kind: harness
        ref: ".planning/scripts/verify-review-result.mjs f20-native-16 (native_macos+native_windows PASS, deferred []) + f20-native-crossaudit; end-to-end validated in a temp git repo (2 positive, 2 negative)"
        status: pass
    human_judgment: false
    rationale: "The profiles validate a synthetic PASS review with native NOT deferred, fail closed when native is deferred under f20-native-16, and leave f20-16 unchanged. The actual 20-26 review blob is produced by the non-author reviewer at 20-26."

# Metrics
duration: ~65min
completed: 2026-07-23
status: complete
---

# Phase 20 Plan 22: macOS Harness Re-Validation + Self-Hosted msvc Pin + UAT-Proof Writer Parity + Native Review Profiles Summary

**Re-validated the macOS proof harness against the real macOS test surface and applied a filename-independent wrong-OS anti-drift guard (the same fail-closed pattern that repaired the Windows harness); pinned the Windows candidate native leg to the AppContainer-capable self-hosted msvc runner; closed the UAT-proof writer/verifier gap by adding the sole durable request writer with exact-tuple idempotency; and added the native-inclusive `f20-native-16` review profile (native NOT deferred) plus the `f20-native-crossaudit` profile — construction-proven on the Mac and node-self-tested, Hetzner clippy clean; all macOS/Windows RUNTIME green explicitly deferred to the 20-25 native-proof gate. No Rust was touched and no Phase 20 requirement is completed.**

## Candidate Tuple

- **source_sha:** `045a947fc1fe9f81f4cbb9d90881f41c5439043c` — tree `09ec531c057933e2ea491993c45e2d74a1bdbccf`
- **inherited (20-21):** `source_sha=9cf5666e`; prior HEAD `c2b26c02`; pristine base `be84bd2`
- **task_base (scope authority):** captured at `c2b26c0266ae8deb2661d07effcf60093db7df3b`, tree `c3167f66`, generation `g-cfd2568a…`
- **Touched paths (scope-verified, exactly 4):**
  - `scripts/f20-native-macos-proof.sh`
  - `.github/workflows/nightly-windows-soak.yml`
  - `scripts/f20-native-uat-proof.mjs`
  - `.planning/scripts/verify-review-result.mjs`
  - **No Rust (production or test) changed.** `git diff --stat` = 4 files, +307/-22.

## Task Commits

1. **Task 1: Re-validate macOS harness + apply wrong-OS anti-drift guard** — `a5a8b737` (fix)
2. **Task 2: Pin self-hosted msvc runner, close writer gap, add review profiles** — `045a947f` (fix)

## Accomplishments

- **REQ-native-r9 (macOS harness re-validation + anti-drift guard):** All 8 `run_target` selectors were audited against the real test surface. Every macOS-specific target resolves to an existing `#![cfg(target_os = "macos")]` test:
  - `macos-retained-directory` → `required_live_macos_retained_directory_confines_writes` in `live_integrity_macos.rs`
  - `macos-process-tree` → `required_live_macos_process_tree_contains_descendants` in `hard_process_containment_macos.rs`
  - the six `any` targets → `docker_smoke.rs`, the `wcore-swarm` lib refusal test, `workspace_authority.rs`, and `transactional_delegated_mutation_test.rs`, all present.
  A bash `assert_target_os_gate` (the macOS mirror of the 20-21 PowerShell `Assert-TargetOsGate`, bash-3.2-safe) runs before every target: an `os=macos` target's selected test must resolve to a source cfg-gated for macOS (positive, load-bearing) and not for a foreign OS (negative); `any` targets are exempt. Source is resolved filename-independently (by the `--test` binary or the file that DEFINES the selected `test(<fn>)`), and gating is on cfg ATTRIBUTES only, never doc-comment prose. The exact 07-22 failure mode (a macОS target pointed at the Windows-only retained-handle test → 0 tests → `--no-tests=fail`) now fails closed at the guard before cargo runs.
- **REQ-native-r11 (self-hosted msvc pin):** the candidate-mode `f20-windows-candidate` job `runs-on` moved from hosted `windows-2022` (AppContainer unavailable) to `[self-hosted, Windows, X64, msvc]`. Scheduled soak (`windows-2022`), the macOS ephemeral pin + labels, `contents: read`-only grant, no-secrets, and no-cache posture are unchanged.
- **Writer/verifier parity (STATE writer-gap):** `f20-native-uat-proof.mjs` gained the sole durable request writer — `persistRequest` (read-existing → `reconcileRequest` → canonical write) backed by a no-follow, mode-0600, fsync'd `writeExactBytesNoFollow`, plus a direct-execution-only `request` subcommand that resolves a Git-private path via `git rev-parse --git-path`. Exact-tuple re-request is byte-identical idempotent; conflicting/malformed/non-pending existing objects fail closed. 20-27 preparation now persists exactly what 20-28 authenticates.
- **REQ-native-r13 (native-inclusive review profiles):** `verify-review-result.mjs` gained `f20-native-16` (`checks: all_severity, asvs_level_2, code_review, native_macos, native_windows, phase_validation`; `deferred: []`) and `f20-native-crossaudit` (`checks: all_severity, evidence_integrity, integration_authority`; `deferred: [native_macos, native_windows]`). Existing `f20-09/11/14/15/16` are untouched.

## Gate Results

### Mac construction / node self-test (allowed operations only)
- **Task 1 — `bash -n`:** syntax OK. **grep guard `! grep 'live_integrity'`:** PASS (no substring). **scope:** `scope-ok base=c2b26c02 paths=1`.
- **Task 1 — anti-drift guard construction proof (guard fn extracted, run against the real repo):** 8/8 — real macos targets (retained-directory by function, process-tree by `--test`) PASS; `any` exempt; and all fail-closed cases (macos→windows-only test, macos→Linux bubblewrap test, macos→nonexistent function, macos→no `-p` crate, unknown os) fail closed with precise messages.
- **Task 2 — `node --check`** on both `.mjs`: OK. **greps:** `msvc` present in the workflow; `f20-native-16` and `f20-native-crossaudit` present in the verifier.
- **Task 2 — `node --test scripts/f20-native-uat-proof.test.mjs`:** **34 passed / 0 failed** (existing suite unchanged).
- **Task 2 — writer parity construction proof (ad-hoc, not committed — test file is out of scope):** 7/7 — fresh durable write (canonical bytes, mode 0600), byte-identical idempotent re-request, conflicting-tuple fails closed without mutating persisted bytes, drifted-commit fails closed, non-pending existing fails closed, symlink authority path fails closed, malformed request rejected.
- **Task 2 — `request` CLI end-to-end (temp git repo):** persists to `.git/f20-native-uat/<commit>/request.json` (mode 600), idempotent identical output, conflicting nonce exits 1, unknown subcommand exits 2.
- **Task 2 — review profiles end-to-end (temp git repo):** `f20-native-16` validates a native-NOT-deferred PASS review; `f20-native-crossaudit` validates with native deferred; `f20-native-16` fails closed when `native_windows` is deferred; `f20-16` still validates unchanged.
- **Task 2 — workflow YAML:** parses; `f20-windows-candidate.runs-on == [self-hosted, Windows, X64, msvc]`; scheduled soak still `windows-2022`; macОS candidate ephemeral pin intact; candidate perms `contents: read`.
- **scope (all 4 declared paths):** `scope-ok base=c2b26c02 paths=4`.

### Hetzner Linux (committed-HEAD authoritative, build-clone HEAD `045a947f`)
- **`vx cargo clippy -p wcore-sandbox -p wcore-swarm --all-targets --all-features -- -D warnings`:** **EXIT=0** (clean).

Note: no Rust changed in this plan, so no compiled surface is affected and the known-slow `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` Linux flake never runs here — noted, not fixed, not a blocker.

## Proven-here vs. deferred-to-20-25

- **Proven here (Mac + node + Hetzner Linux):** all four artifacts are constructed and their *premises/logic* verified — the macOS anti-drift guard's positive/negative gates and fail-closed behavior (8/8 cases), the exact msvc `runs-on` pin (YAML-asserted), the request writer's durability + idempotency + fail-closed parity (7/7 + CLI end-to-end), and both review profiles (4 end-to-end cases incl. negatives). Hetzner clippy is clean at the committed HEAD; no Rust changed, so there is no Linux regression.
- **UNPROVEN here, explicitly deferred to the native-proof gate (20-25):** every macOS/Windows RUNTIME claim. macOS `cargo nextest`/live-docker execution runs ONLY on the Scaleway Apple-silicon runner (which requires `DOCKER_HOST`/colima exported into the runner env). The self-hosted msvc runner's actual AppContainer availability and the Windows Job-Object green are observed ONLY when the candidate leg runs at 20-25. The macOS-side `assert_target_os_gate` runtime execution is likewise observed at 20-25 (the guard was construction-run on the Mac here, but the full harness preflight requires Darwin + acceptance env + a clean exact-commit checkout + live docker). **No native claim is made from Mac inspection, Linux, or source review.**

## Explicit non-claims

- No macOS or Windows native green is claimed from this Mac/Linux run.
- No Phase 20 requirement is marked complete. REQ-native-r9/r11/r13 exist as reviewable, gated, construction-proven artifacts plus a Linux no-regression result.
- No aggregate / native / phase claim is made.

## Deviations from Plan

- **[Rule 3 — blocking verify literal reconciled] macos-retained-directory selects by function name, guard is filename-independent.** The plan's Task-1 verify `! git grep -q 'live_integrity' -- scripts/f20-native-macos-proof.sh` is a substring guard intended to forbid the 07-22 Windows-only `live_integrity.rs` mis-map, but it also matches the legitimately-named repaired macOS file `live_integrity_macos.rs` (created by prior plan `3383a46d`) — and it matched twice on the current script (a doc comment naming `tests/live_integrity.rs`, and the `--test live_integrity_macos` selector). The 20-21 Windows precedent used a *positive* grep for the repaired file, not a broad negative one. Renaming the Rust test file to eliminate the substring is outside this plan's declared 4-file `files_modified` scope. The honest in-scope resolution: reword the comment, and select the macOS retained-directory test by its macOS-specific function name (`required_live_macos_retained_directory_confines_writes`, no `live_integrity` substring) via `-E 'test(...)'` without `--test`, with the anti-drift guard resolving each target's source by the file that DEFINES the selected function. This satisfies the verify literal honestly (not by obfuscation), keeps the harness fail-closed, and is a stronger, filename-independent form of the 20-21 guard. Recorded for 20-25/replan: consider tightening the guard literal to a precise negative (e.g. forbid only a bare `--test live_integrity ` selector) or renaming the Rust test in a follow-up whose scope includes it.
- **[Scope-bounded] Writer self-tests not committed.** Writer/verifier parity was construction-proven via ad-hoc node scripts (7/7 + CLI + profile end-to-end), NOT by editing `scripts/f20-native-uat-proof.test.mjs`, which is not in this plan's `files_modified`. The committed self-test suite (34 cases) stays green and unchanged. A follow-up whose scope includes the test file should fold the writer cases into it.

## Self-Check: PASSED

- `scripts/f20-native-macos-proof.sh` — FOUND (assert_target_os_gate added + wired into run_target with per-target os; retained-directory selects by function name; no `live_integrity` substring)
- `.github/workflows/nightly-windows-soak.yml` — FOUND (f20-windows-candidate runs-on = [self-hosted, Windows, X64, msvc]; scheduled soak + macOS leg unchanged)
- `scripts/f20-native-uat-proof.mjs` — FOUND (persistRequest + writeExactBytesNoFollow + serializeRequest + `request` subcommand; existing exports/suite unchanged)
- `.planning/scripts/verify-review-result.mjs` — FOUND (f20-native-16 + f20-native-crossaudit profiles; existing profiles unchanged)
- Task 1 commit `a5a8b737` — FOUND in `git log`
- Task 2 commit `045a947f` — FOUND in `git log`
- Hetzner Linux at HEAD `045a947f`: clippy `-D warnings` EXIT=0; node --test 34/0.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-23*
*Construction-only; macOS/Windows native green deferred to 20-25; no requirement completed.*
