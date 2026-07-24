---
phase: 20-transactional-delegated-mutation
plan: "50"
subsystem: infra
tags: [native-repair, finding-fix, further-repaired-successor, four-way-panel-followup, containment-assertion, reap-non-vacuous, captured-pid, cmd-classifier, exact-component, windows-only, cfg-windows, no-regression-floor, honest-deferral]

# Dependency graph
requires:
  - phase: "20-45"
    provides: "FOUR-WAY pre-native panel BLOCK on sealed-but-RED 3f839309574d6741eed416cd3820f56447f74eba (tree 3092475bb4102d010b6ff5f6c9d8080cb4f51928), with two real, statically-provable findings: (F1, Codex HIGH / Claude MEDIUM) active_process_cap_is_enforced's post-close reap assertion is vacuous once the tagged parent cmd dies; (F2, Codex MEDIUM / Claude LOW) resolved_program_is_cmd uses ends_with(cmd.exe) which suffix-matches notcmd.exe."
provides:
  - "FURTHER-REPAIRED SUCCESSOR candidate source_sha f0dd5b6d312af616f268f96f34c3bc9fc962c4d2 (tree ac76c87b318ee4ba8c34927dea23e40e63fd0776) over the sealed-but-RED 3f839309: both 20-45-panel findings closed, touching ONLY the two files the findings name."
  - "Finding 1 CLOSED (threads the needle): the post-close reap check captures the fan-out choice PIDs (tagged_choice_descendant_pids) while the tagged parent is alive, then re-checks those exact ProcessIds after job close by fixed PID intersected with image choice.exe (surviving_captured_choice_pids) — non-vacuous (a leaked/orphaned captured survivor is still counted) AND not host-wide-flaky (a concurrent live_fs_acl choice.exe carries a different, non-captured PID). Does NOT revert to the 20-43 Task 3 host-wide image_count flake. Every other containment assertion (peak>0, peak<=LIMIT, peak<attempts, exit 0, reap_stray_choice) and the sibling job_close_reaps_detached_descendant_with_no_residue / tagged_cmd_count preserved verbatim."
  - "Finding 2 CLOSED: resolved_program_is_cmd matches cmd.exe as the exact FINAL PATH COMPONENT (std::path::Path::file_name, case-insensitive) rather than a bare suffix, so notcmd.exe/foocmd.exe no longer classify as cmd; quote_cmd_payload/quote_arg/classify_bare_shell/resolve_program/is_unc_or_device_path byte-identical (the #B quoting behavior is unchanged)."
  - "NO-REGRESSION floor green: sanctioned macOS cargo check -p wcore-sandbox --features live-docker --tests exit 0; Hetzner clippy -D warnings exit 0; Hetzner aggregate nextest --workspace --profile ci --no-fail-fast = 11509/0/48 (exit 0). Both fixes are #![cfg(windows)] / windows_impl — zero Linux compiled-output change, so the floor is unchanged, not a proof of the fixes."
affects: ["20-51", "20-52", "20-53"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Two-phase containment reap check for a parent-scoped process query: capture the target PIDs by ParentProcessId WHILE the tagged parent is alive (immune to a concurrent same-image target), then re-verify those exact PIDs by fixed ProcessId AFTER the parent dies — because the parent-scoped query goes structurally empty once the parent is reaped. Threads the needle between a vacuous parent-scoped post-close check and a host-wide-image-count flake."
    - "Classify a resolved Windows image by its exact final path component (Path::file_name == cmd.exe, case-insensitive) rather than a bare suffix, so a sibling-named image (notcmd.exe) cannot be misrouted through image-specific quoting."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-50-SUMMARY.md
  modified:
    - crates/wcore-sandbox/tests/hard_process_containment_windows.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs

key-decisions:
  - "Finding 1 fix threads the needle exactly as the 20-45 routing demanded. tagged_choice_descendant_count(-> usize) is replaced by tagged_choice_descendant_pids(-> Vec<u32>): the SAME ParentProcessId-scoped CIM query (Select-Object -ExpandProperty ProcessId on the choice.exe children of the tagged cmd), returning the PID list. The peak-sampling loop now captures the peak PID set (captured_pids) alongside peak, preserving the early-break/500ms-overshoot-resample shape. After the three peak assertions and the clean-exit assertion (all verbatim), a belt-and-braces assert!(!captured_pids.is_empty()) guards against silent re-vacuity, then the final wait_until uses surviving_captured_choice_pids(&captured_pids)==0 — a NEW helper that counts choice.exe whose ProcessId is in the captured set (empty slice short-circuits to 0, no malformed filter). This detects a leaked/orphaned captured survivor by fixed PID (non-vacuous) while a concurrent target's choice.exe is excluded by its non-captured PID (no host-wide flake). The 20-43 Task 3 flake fix is preserved, NOT reverted."
  - "Finding 2 fix is an exact-final-component match. resolved_program_is_cmd keeps the NUL-terminated UTF-16 decode + lowercasing, then extracts the final component via std::path::Path::new(&decoded).file_name() (Windows Path treats both \\ and / as separators, covering the System32\\cmd.exe resolution and any absolute path) and compares == cmd.exe. Only an image whose filename IS cmd.exe classifies as cmd. quote_cmd_payload/quote_arg/classify_bare_shell/resolve_program/is_unc_or_device_path are byte-identical (git diff 3f839309 for command.rs shows +/- lines ONLY inside resolved_program_is_cmd and its doc)."
  - "OPTIONAL inline #[cfg(test)] classifier assertion in command.rs was SKIPPED (executor judgment). The codebase's home for windows_impl helper unit tests is the sibling windows_impl/tests.rs (e.g. is_verbatim_disk_path_classifies_prefixes), which is NOT one of the two declared files and is forbidden to touch. Adding a competing inline test module to command.rs would break that convention and the surgical-change rule for no verifiable benefit before 20-53 (the whole file is #[cfg(windows)], so any such test only compiles at the msvc re-dispatch anyway). The classifier tightening itself is the fix."
  - "HONESTY (CONTEXT D5/D7): both edited files are #![cfg(windows)] (hard_process_containment_windows.rs) / part of windows_impl #[cfg(windows)] (command.rs). Neither this Mac nor Hetzner Linux compiles a single line of the two fixes. Their COMPILE and real-hardware BEHAVIOR (non-vacuous reap; tightened cmd classifier) are proven ONLY at the 20-53 self-hosted msvc re-dispatch. The macOS cargo check and the Hetzner clippy+aggregate are a NO-REGRESSION floor (they must stay green + 11509/0/48), not a proof of the Windows fixes."
  - "Re-dispatch structure (from 20-50-PLAN redispatch_decision): the further-repaired successor is the additive serial chain 20-50 (fix, THIS plan) -> 20-51 (re-seal / lock no-op confirm) -> 20-52 (four-way pre-native cross-audit) -> 20-53 (native re-dispatch) -> 20-54 (native-inclusive review) -> 20-55 (re-prep) -> 20-56 (terminal). 20-43/20-44 stay the sealed record; 20-45 stays the BLOCK gate; 20-46...20-49 are SUPERSEDED (dead — their bound candidate 3f839309 is terminal RED) and were NEITHER executed NOR edited by this chain."

patterns-established:
  - "A parent-scoped host-side liveness query used for a post-close containment assertion must NOT be reused after the scoping parent dies — it goes structurally empty and the assertion becomes vacuous. Capture the concrete target PIDs while the parent is alive and re-verify by fixed PID after close."

requirements-completed: []  # NO Phase-20 requirement is claimed. Both fixes are Windows-only; their compile+behavior are deferred to the 20-53 msvc re-dispatch. This plan implements REQ-native-r2/r5/r7/r4 as fixes but completes no requirement.

# Metrics
duration: ~35min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 50: Close the two 20-45-panel findings on the sealed-but-RED 3f839309 — further-repaired successor Summary

**The two real, statically-provable findings the 20-45 FOUR-WAY pre-native panel (Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3 + internal Claude adversarial) returned BLOCK on against sealed candidate `3f839309` are now CLOSED as a clean further-repaired successor `f0dd5b6d` (tree `ac76c87b`) over the exact sealed tree, touching ONLY the two files the findings name. Finding 1 (vacuous post-close reap) threads the needle — non-vacuous by captured ProcessId, without reintroducing the host-wide-image-count flake 20-43 Task 3 removed. Finding 2 (loose cmd suffix) is an exact final-path-component match. Both files are `#![cfg(windows)]`/`windows_impl`; their compile AND real-hardware behavior are HONESTLY DEFERRED to the 20-53 msvc re-dispatch. The Mac + Hetzner gates are a NO-REGRESSION floor and stayed green + 11509/0/48.**

## New candidate identity

- **source_sha:** `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`
- **source_tree:** `ac76c87b318ee4ba8c34927dea23e40e63fd0776`
- **predecessor (sealed-but-RED):** `3f839309574d6741eed416cd3820f56447f74eba` (tree `3092475bb4102d010b6ff5f6c9d8080cb4f51928`, verified `3f839309^{tree}` == `3092475b`)
- **branch:** `plan/f20-unified-audit-repair` (isolated STANDALONE checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`).
- **base at plan start:** `db16d18c7ab6662455397fd783a3c076f51606c0` (only planning-doc commits sit between the seal and this base).

### Pre-edit byte-identity vs the sealed tree
`git diff 3f839309 -- crates/wcore-sandbox/tests/hard_process_containment_windows.rs crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs` was **EMPTY** (exit 0) before any edit — no source drift; the fixes were authored on top of the exact sealed content.

## Task commits

| Task | Commit | Type | What |
|---|---|---|---|
| 1 | `472d7c03` | test | Finding 1 — non-vacuous post-close reap by captured `choice` PID in `active_process_cap_is_enforced` |
| 2 | `f0dd5b6d` | fix | Finding 2 — `resolved_program_is_cmd` matches `cmd.exe` as the exact final path component |

## Finding 1 — non-vacuous post-close reap (threads the needle)

**File:** `crates/wcore-sandbox/tests/hard_process_containment_windows.rs`

- **The defect:** the final `wait_until(|| tagged_choice_descendant_count(&tag) == 0, ...)` scoped `choice.exe` by whose `ParentProcessId` is a live tagged `cmd.exe`. After `run.await` returns the job has closed and the tagged parent cmd is dead → the `cmd.exe`-with-tag query is empty → `$parents` is empty → the count is structurally 0 whether or not a `choice` leaked. Vacuous.
- **The fix:**
  1. `tagged_choice_descendant_count(&tag) -> usize` replaced by `tagged_choice_descendant_pids(&tag) -> Vec<u32>` — the SAME `ParentProcessId`-scoped CIM query, but `Select-Object -ExpandProperty ProcessId` on the `choice.exe` children (returning the PID list), parsed to `Vec<u32>`. Doc rewritten to the two-phase (alive-capture + post-close-by-PID) design; the inaccurate "without weakening any containment assertion" line (which the 20-45 panel flagged) is DROPPED.
  2. New `surviving_captured_choice_pids(pids: &[u32]) -> usize`: counts `choice.exe` whose `ProcessId` is in the captured set (`$pids -contains $_.ProcessId`); an empty slice short-circuits to `0` (no malformed filter). Matches ONLY the test's own captured PIDs intersected with image `choice.exe`.
  3. The peak-sampling loop captures the peak PID set (`captured_pids`) alongside `peak` (`pids.len() > peak` → update both), preserving the early-break + 500ms-overshoot-resample shape (the overshoot also updates `captured_pids` if larger).
  4. The three peak assertions (`peak > 0`, `peak <= SANDBOX_ACTIVE_PROCESS_LIMIT`, `peak < attempts`) and the `exit_code == 0` assertion are verbatim; a belt-and-braces `assert!(!captured_pids.is_empty(), ...)` guards re-vacuity; the final reap check is `wait_until(|| surviving_captured_choice_pids(&captured_pids) == 0, 30, "fan-out descendants reaped after job close (by captured PID)")`; the trailing `reap_stray_choice()` is kept.
- **Threads the needle:** non-vacuous (a leaked/orphaned captured `choice` carries the same PID and is still `choice.exe`, so it is counted) AND not host-wide-flaky (a concurrent `live_fs_acl` `choice.exe` carries a different, non-captured PID and is excluded). It does NOT revert to the host-wide `image_count("choice.exe")` baseline the 20-43 Task 3 flake fix removed.
- **Preserved verbatim:** every other containment assertion; the sibling `job_close_reaps_detached_descendant_with_no_residue` (robust reap-property cover — its grandchild carries `rem {tag}` on its OWN command line so `tagged_cmd_count` survives parent death); `tagged_cmd_count`. Grep-confirmed the old `tagged_choice_descendant_count` has **0** remaining references (no dead code). Diff: 88 insertions / 18 deletions, one file.

## Finding 2 — exact final-component cmd classifier

**File:** `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs`

- **The defect:** `resolved_program_is_cmd` used `.ends_with("cmd.exe")`, which suffix-matches `notcmd.exe`/`foocmd.exe` and would route their `/c`/`/k` payload through the cmd-specific `quote_cmd_payload` instead of `quote_arg`.
- **The fix:** keep the NUL-terminated UTF-16 decode + lowercasing, then extract the final component via `std::path::Path::new(&decoded).file_name()` (Windows `Path` treats both `\` and `/` as separators, covering the `System32\cmd.exe` resolution and any absolute path) and compare `== "cmd.exe"`. Only a filename that IS `cmd.exe` classifies as cmd. Doc updated to the exact-final-component wording.
- **Byte-identical elsewhere:** `git diff 3f839309 -- command.rs` shows `+`/`-` lines ONLY inside `resolved_program_is_cmd` and its doc comment; `quote_cmd_payload` / `quote_arg` / `classify_bare_shell` / `resolve_program` / `is_unc_or_device_path` are unchanged (grep-confirmed present; the only `quote_cmd_payload` match in the diff is a hunk-context header, not a change). The #B quoting behavior is unchanged — only the cmd/non-cmd classification is tightened. Diff: 12 insertions / 4 deletions, one file.
- **Optional inline test:** SKIPPED (executor judgment) — the codebase's home for these helper unit tests is the sibling `windows_impl/tests.rs`, which is not a declared file; adding a competing inline test module to `command.rs` would break convention and the surgical-change rule with no benefit before the 20-53 msvc compile.

## Gate receipts (NO-REGRESSION floor)

Because both fixes are Windows-only, these gates prove **no Linux/macOS regression**, NOT the Windows fixes.

| Gate | Command | Result |
|---|---|---|
| macOS compile (sanctioned) | `cargo check -p wcore-sandbox --features live-docker --tests` | **exit 0** — `cargo build: 0 errors, 1 warnings (1 crate)`. The 1 warning is a pre-existing dead-code `signal` in `process_tree.rs` (macOS branch, NOT a touched file) — out of scope, logged not fixed. |
| Hetzner clippy | `remote-cargo.sh f20-50-clippy <checkout> clippy -p wcore-sandbox --all-targets --all-features -- -D warnings` | **exit 0** — clean, `Finished dev profile in 10.86s`, committed HEAD `f0dd5b6d`, slot `/root/cargo-slots/f20-50-clippy`. |
| Hetzner aggregate | `remote-cargo.sh f20-50-aggregate <checkout> nextest run --workspace --profile ci --no-fail-fast` | **exit 0** — `Summary [69.171s] 11509 tests run: 11509 passed (2 flaky), 48 skipped` = **11509/0/48** (0 FAIL, 48 SKIP, 2 known-flaky retried green), committed HEAD `f0dd5b6d`, slot `f20-50-aggregate`. Runs `--locked` (20-50 changes no manifest; the lock is unchanged — 20-51 confirms zero lock delta). |

The macOS `cargo check` re-confirms the macOS-compiled `wcore-sandbox` surface still compiles — it does NOT compile the `#![cfg(windows)]` containment test or the `windows_impl` classifier. The Hetzner aggregate stays **exactly 11509/0/48** — unchanged from the sealed baseline (run 32f1b4ba), the honesty gate that a Windows-only delta introduces zero Linux compiled-output change.

## Deviations from Plan

**1. [Rule 3 — Blocking, tooling] Restored the pre-existing untouched AGENTS.md ijfw-memory churn to its committed state.** The isolated checkout carried a pre-existing, non-task `AGENTS.md` modification (ijfw auto-detection metadata churn: `confidence`/`detected_at`/signal counts — regenerable, HEAD holds a valid version). `verify-task-scope.sh --capture` / `--start-fresh` / `--complete` all call `require_clean_checkout`, which rejects any dirty file, and the per-task scope union folds in the unstaged diff — so the churn would both block the base capture and be flagged out-of-scope. Restored it with `git checkout -- AGENTS.md` (single-file, non-destructive — the sanctioned restore, not a blanket reset; this is a STANDALONE checkout, not a worktree) before each scope-gate, and re-restored when ijfw re-churned it. **AGENTS.md was NEVER staged into any commit** — the two task commits stage only their one declared file individually — so "touch ONLY the two declared files" holds for the committed delta. No source/test/workflow change.

**2. [Judgment — not a code deviation] Optional inline classifier test skipped.** See Finding 2 above — deferred to keep the delta surgical and honor the two-file scope.

No scope creep — only the two declared source files (in their task commits) and this summary + the standard planning docs (in the metadata commit) were written.

## Explicit deferral to 20-53

ALL Windows compile AND behavior are DEFERRED to the 20-53 self-hosted msvc re-dispatch:
- **Finding 1:** that the reap check is non-vacuous (catches a leaked/orphaned captured `choice` survivor) and not host-wide-flaky (excludes a concurrent target's `choice.exe`), and that the three peak assertions + sibling reap test still hold, are proven ONLY when `hard_process_containment_windows.rs` actually compiles and runs on the AppContainer msvc runner.
- **Finding 2:** that `resolved_program_is_cmd` classifies `...\System32\cmd.exe` as cmd and `...\notcmd.exe` as not-cmd, with the #B quoting behavior intact, is proven ONLY when `windows_impl` compiles on msvc.

No native, aggregate-seal, or requirement claim is made here. The re-seal is 20-51; the four-way re-audit is 20-52; the native re-dispatch (under a fresh Sean authorization) is 20-53.

## Self-Check: PASSED

- Both touched files exist and are the only two source files modified (`git diff 3f839309 --stat` scope: exactly the two declared files).
- Task commits present: `git log --oneline` shows `472d7c03` (test) and `f0dd5b6d` (fix) on `plan/f20-unified-audit-repair`.
- New candidate `source_sha` `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`, tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776` (verified `HEAD^{tree}`).
- Finding 1 helpers grep-confirmed: `tagged_choice_descendant_pids`, `surviving_captured_choice_pids`, `captured_pids`, `ExpandProperty ProcessId`, `-contains ... ProcessId`; three peak assertions present; old `tagged_choice_descendant_count` has 0 references; sibling reap test + `tagged_cmd_count` untouched.
- Finding 2 grep-confirmed: `resolved_program_is_cmd` present, `file_name` present; `quote_cmd_payload`/`is_unc_or_device_path`/`SANDBOX_ACTIVE_PROCESS_LIMIT` present and unchanged.
- No Phase 20 requirement claimed.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
