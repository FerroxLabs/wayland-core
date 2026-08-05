---
phase: 20-transactional-delegated-mutation
plan: "43"
subsystem: infra
tags: [native-repair, windows, dunce, verbatim-path, cmd-quoting, appcontainer, job-object, capacity-probe, dispatch-admission, wcore-swarm, wcore-sandbox, hetzner-proof, remote-cargo, third-repaired-successor, no-linux-regression]

# Dependency graph
requires:
  - phase: "20-37"
    provides: "Sealed-but-RED second-repaired successor daf27337 (tree 91de96a3); the full --no-fail-fast Windows diagnostic (run 30064412019) + Kimi K3 cross-audit collapsed its 5 RED targets into the two PRODUCTION root causes (#A verbatim-path probe reparse, #B cmd.exe quote-mangling) this plan fixes"
provides:
  - "Third-repaired successor candidate source_sha 92cac8bb0a950082b92de9698c15ccde4eab9e44 (tree 0fd3368b1ad08888409cf08560d95347d9d5b851) carrying the two production #A/#B fixes plus the Job-Object counting-scope and lib-test compile-debt fixes"
  - "#A (Linux-exercised): worktree_manager de-verbatimizes both canonicalize sites via dunce::simplified (no-op on unix) + env-var (WCORE_SWARM_PROBE_ROOT) capacity-probe transport, DispatchAdmission budget preserved; dunce promoted to a direct dep (already in Cargo.lock, no new crate)"
  - "#B (Windows-only, proven at 20-46): cmd.exe /c|/k payload wrapped in a single outer quote pair (inner quotes verbatim) so cmd /s runs the script correctly; argv discipline + sandbox boundary preserved"
  - "Job-Object choice counting scoped to the test's tagged ParentProcessId tree (+ belt-and-braces cfg(windows) nextest serialization group); lib-test imports restored + Wdk FILE_ID path corrected"
  - "Receipts: sanctioned macOS cargo check GREEN; Hetzner clippy (wcore-swarm+wcore-sandbox, -D warnings) GREEN; Hetzner aggregate proven NO 20-43 Linux regression (the sole aggregate failure reproduces identically at the base 4ca0a8f7)"
affects: ["20-44", "20-45", "20-46", "20-47", "20-48", "20-49"]

# Tech tracking
tech-stack:
  added:
    - "dunce (1, already resolved 1.0.5 in Cargo.lock) — promoted from transitive to a direct dependency of wcore-swarm; no new crate pulled"
  patterns:
    - "De-verbatimize Windows canonical paths at the canonicalize SITE (dunce::simplified) so the fix defuses the whole \\?\ class in one place (probe + downstream git), no-op on unix"
    - "Out-of-band env-var transport (.env on the tokio Command) for a PowerShell -Command probe argument, removing the trailing-arg reparse class without weakening the admission budget"
    - "cmd.exe /c|/k payload quoting distinct from MSVC-CRT quote_arg: single outer pair, inner quotes verbatim, because cmd /s re-reads the RAW command line"
    - "Scope a host-side process count to the test's OWN tagged process tree (by ParentProcessId of a rem {tag}-carrying parent) instead of host-wide image name, closing concurrent-runner pollution"
    - "cfg(windows)-gated nextest test-group serialization so a Linux --profile ci aggregate is provably untouched"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-43-SUMMARY.md
  modified:
    - crates/wcore-swarm/src/worktree_manager.rs
    - crates/wcore-swarm/Cargo.toml
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs
    - crates/wcore-sandbox/tests/hard_process_containment_windows.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs
    - crates/wcore-sandbox/src/directory_authority_windows_tests.rs
    - .config/nextest.toml

key-decisions:
  - "Cargo.lock was NOT committed. Task 1 adds `dunce` to wcore-swarm/Cargo.toml; the sanctioned macOS `cargo check` resynced the lock (a single `+ \"dunce\"` edge under the wcore-swarm [[package]] dependencies — the dunce 1.0.5 package stanza already existed, so NO new crate). Per the plan the lock resync is 20-44's authoritative concern and the aggregate runs WITHOUT --locked, so the lock edit was reverted (git checkout HEAD -- Cargo.lock) and the committed HEAD carries the base lock; the remote re-resolves the edge on the fly. No package-legitimacy checkpoint triggered (T-20-43-05: no new crate)."
  - "The FILE_ID_BOTH_DIR_INFORMATION import path in directory_authority_windows_tests.rs was corrected from the wrong Win32::Storage::FileSystem to Wdk::Storage::FileSystem (windows-sys 0.59, feature Wdk_Storage_FileSystem) — matching production directory_authority_windows.rs:12. FILE_RENAME_INFO stays on Win32."
  - "image_count(\"choice.exe\") (host-wide by image) was replaced by tagged_choice_descendant_count (scoped by ParentProcessId of a rem {tag}-carrying sandbox parent) and removed entirely, so no dead code remains for the 20-46 msvc clippy build. tagged_cmd_count was left unchanged: it is already tag-scoped and not vulnerable."
  - "The nextest serialization group is gated platform = 'cfg(windows)' so the Hetzner --profile ci aggregate on Linux never applies it (the two native targets compile to empty binaries on Linux) — the counting-scope fix is the real correctness guarantee, the group is belt-and-braces."

patterns-established:
  - "Per-task disjoint scope gate advanced with verify-task-scope.sh --start-fresh between the four provisional task commits"
  - "Honest cfg(windows) deferral: process.rs/command.rs/containment/lib-test compile AND behavior are proven ONLY at the 20-46 msvc re-dispatch; no pre-dispatch gate claims a Windows fix verified"

requirements-completed: []  # NO Phase-20 requirement is claimed here (that is 20-49's job).

# Metrics
duration: ~90min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 43: Windows-hardening production fixes (#A verbatim-path probe, #B cmd quote-mangling) + Job-Object counting scope + lib-test compile debt Summary

**Third-repaired successor over the sealed-but-RED daf27337: de-verbatimizes swarm roots with dunce::simplified + env-var capacity-probe transport (#A, Linux-exercised), fixes cmd.exe /c|/k payload quoting for cmd's raw re-read (#B, Windows-only), scopes Job-Object choice counting to the test's tagged ParentProcessId tree, and restores the two lib-test modules' imports — macOS check + Hetzner clippy GREEN, and the aggregate's sole failure is PROVEN pre-existing (identical at base 4ca0a8f7), so 20-43 adds NO Linux regression.**

## Candidate identity

- **source_sha:** `92cac8bb0a950082b92de9698c15ccde4eab9e44`
- **tree:** `0fd3368b1ad08888409cf08560d95347d9d5b851`
- **branch:** `plan/f20-unified-audit-repair` (isolated standalone checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory)
- **base (sealed-but-RED):** `daf27337` — only planning-doc commits sit between it and this branch tip; the 8 touched files were byte-identical to the sealed tree before editing (no source drift).

## Performance

- **Duration:** ~90 min
- **Tasks:** 4 (all committed atomically)
- **Files modified:** 8

## Accomplishments
- **#A (production, Linux-exercised):** `worktree_manager.rs` wraps both `std::fs::canonicalize` sites (`repo_root`, `swarm_root`) with `dunce::simplified` so the stored roots are ordinary drive-letter paths on Windows (no-op on unix), defusing the `\\?\`-verbatim class at the source (also the latent downstream-git watch-item d1). The Windows capacity probe now transports the root via the `WCORE_SWARM_PROBE_ROOT` env var and reads `$env:WCORE_SWARM_PROBE_ROOT`, dropping the trailing `-Command` arg that PowerShell re-parsed as script text. The DispatchAdmission budget is preserved — real free-space computation intact, **no** capacity-to-unlimited fallback (grep-confirmed absent). `dunce` promoted to a direct dep (already in Cargo.lock; no new crate).
- **#B (production, Windows-only — proven at 20-46):** `command.rs` gains `quote_cmd_payload` (single outer quote pair, inner quotes verbatim) + `resolved_program_is_cmd`; `process.rs` uses them for the argument following the first `/c`/`/k` when the resolved program is cmd.exe, leaving `quote_arg` unchanged for every other argv. cmd `/s` strips only the outer pair and runs the script verbatim, so `type "path"` and `start "" /b …` no longer mangle. Quoting-layer only: argv discipline, `is_unc_or_device_path`, the Low-IL restricted token, the Job-Object limits, the AppContainer ACL lease, and the 20-36 `exit /b 0` normalization are all preserved.
- **Job-Object flake (r7):** `hard_process_containment_windows.rs` replaces host-wide `image_count("choice.exe")` with `tagged_choice_descendant_count` — the top-level sandbox cmd carries a unique `rem {tag}` and every `start /b` choice idler is its direct child, so the count scopes to `choice.exe` whose `ParentProcessId` is this test's tagged cmd. A concurrent `choice`-spawning target can no longer pollute the baseline/delta. Every containment assertion is preserved verbatim. Belt-and-braces `cfg(windows)`-gated `native-windows-live` nextest test-group serializes the two native choice-spawning targets.
- **Lib-test compile debt (r3):** `windows_impl/tests.rs` gains `Arc` + crate `SandboxCommand`/`SandboxError`/`SandboxManifest`/`NetworkPolicy` imports; `directory_authority_windows_tests.rs` corrects `FILE_ID_BOTH_DIR_INFORMATION` to `Wdk::Storage::FileSystem` and adds the used `SandboxError` — imports only, no test body changed.

## Task Commits

Each task was committed atomically after passing its Mac construction gate (per-task disjoint scope via `verify-task-scope.sh` + `vx rustfmt --edition 2024 --check` + grep/toml checks):

1. **Task 1: #A de-verbatimize swarm roots + env-var probe transport** — `76da033f` (fix)
2. **Task 2: #B cmd.exe /c|/k payload quoting** — `c9258219` (fix)
3. **Task 3: Job-Object counting scoped to tagged tree + nextest group** — `230e4521` (fix)
4. **Task 4: lib-test imports + Wdk FILE_ID path** — `92cac8bb` (fix)

## Gate receipts

| Gate | Command | Result |
|---|---|---|
| macOS sanctioned compile (CONTEXT D5 carve-out) | `cargo check -p wcore-sandbox --features live-docker --tests` | **GREEN** — exit 0, 0 errors (compiles lib + docker_smoke + macOS-gated tests; does NOT compile the `#![cfg(windows)]` code). The lone warning (`process_tree.rs:403` `MacProcessIdentity::signal` dead_code) is pre-existing and outside the delta — logged, not fixed (out of scope). |
| Hetzner clippy (Linux) | `remote-cargo.sh f20-43-clippy … clippy -p wcore-swarm -p wcore-sandbox --all-targets --all-features -- -D warnings` | **GREEN** — exit 0, zero warnings. Proves the Linux-compiled wcore-swarm `dunce::simplified` change is clean under `-D warnings`. |
| Hetzner aggregate (Linux) | `remote-cargo.sh f20-43-aggregate … nextest run --workspace --profile ci --no-fail-fast` | **11508 passed / 1 failed / 48 skipped** — the single failure (`wcore-agent spawner::production_durable_spawn_tests::concurrent_near_cap_admits_exactly_one_retained_workspace`) is **PROVEN pre-existing / environmental, NOT a 20-43 regression** (see below). |

## HONESTY — all Windows compile + behavior deferred to 20-46

`process.rs`, `command.rs`, `hard_process_containment_windows.rs`, `windows_impl/tests.rs`, and `directory_authority_windows_tests.rs` are `#[cfg(windows)]`/`#![cfg(windows)]` — **neither this Mac nor Hetzner Linux compiles a line of them.** Their compile AND real-hardware behavior (the env-var probe transport, the cmd payload quoting, the containment counting scope, the lib-test compile) are proven ONLY on the self-hosted msvc runner at the **20-46** native re-dispatch. No pre-dispatch gate here claims any Windows fix "verified". Only the wcore-swarm `worktree_manager.rs`/`Cargo.toml` change (`dunce::simplified`, no-op on unix) is Linux-compiled and Linux-exercised — its no-regression is proven on Hetzner below.

## Deviations from Plan

### Auto-fixed / handled issues

**1. [Rule 3 — Blocking] Cargo.lock resynced by the macOS check; reverted to keep committed HEAD lock-consistent**
- **Found during:** verification (macOS `cargo check` after Task 1 added `dunce` to `wcore-swarm/Cargo.toml`).
- **Issue:** `cargo check` added a `+ "dunce"` edge under the `wcore-swarm` `[[package]]` dependencies in Cargo.lock, dirtying the worktree so the committed-HEAD Hetzner harness refused to run (`local tracked worktree differs from the index`).
- **Fix:** Confirmed the delta was ONLY the dunce edge and that the `dunce` 1.0.5 package stanza already existed in the lock (NO new crate), then `git checkout HEAD -- Cargo.lock`. Per the plan the lock resync is 20-44's authoritative concern and the aggregate runs WITHOUT `--locked`, so the committed HEAD carries the base lock and the remote re-resolves the edge on the fly.
- **Verification:** `grep -A3 '^name = "dunce"' Cargo.lock` shows 1.0.5 present pre- and post-; worktree clean after revert; harness preflight passed on retry.
- **No package-legitimacy checkpoint triggered** (T-20-43-05: no genuinely new crate).

**2. [Rule 3 — Blocking, environmental] Aggregate's sole failure is a pre-existing disk-pressure flake, not a 20-43 regression**
- **Found during:** the two Hetzner aggregate runs.
- **Issue:** `wcore-agent spawner::…::concurrent_near_cap_admits_exactly_one_retained_workspace` failed (TRY 3 FAIL, `left: 0 / right: 1` — both concurrent near-cap durable launches lost, when exactly one should win). Different sibling swarm/spawn concurrency tests flaked-then-passed in each run.
- **Root cause (environmental, not code):** the test prefills `MAX_RETAINED_WORKTREES-1` leases then races two launches for the last slot. Admission (`workspace_capacity`) reserves `MAX_TRANSACTION_WORKSPACE_BYTES` (8 GB) + `WORKSPACE_SAFETY_MARGIN_BYTES` (512 MB) per launch and checks it against `df` free space. The Hetzner box is at **97% disk (~52 GB free, hundreds of stale cargo-slots)**, so both near-cap admissions fail closed → `left: 0`. When the box was cleaner (20-37 seal) the same test passed at 11509/0/48.
- **Proof of non-regression (definitive):** the failing file `crates/wcore-agent/src/spawner.rs` is NOT in the 20-43 delta, and the only Linux-compiled 20-43 change (`dunce::simplified`) is a literal unix no-op. The test fails **identically at the base commit `4ca0a8f7`** — 3/3 in isolation — and at this HEAD — 5/5 in isolation, all deterministic ~0.08 s, same `left: 0`. `spawner.rs` is byte-identical between the sealed `daf27337` and `4ca0a8f7` (only docs commits between), so this failure pre-dates 20-43 and is caused by the box's disk state, not by this delta.
- **Disposition:** 20-43 introduces **NO Linux regression** (the honesty gate's actual concern is satisfied). The literal clean `11509/0/48` receipt is **environmentally blocked** on the box's disk pressure and must be re-obtained at **20-44**'s re-seal on a box with adequate free disk — it is NOT a code blocker for this delta.

---

**Total deviations:** 2 handled (1 blocking lock-resync revert, 1 environmental non-regression flake). No scope creep — only the 8 declared files were touched.
**Impact on plan:** The #A/#B/r7/r3 production fixes are complete and correct; the macOS + Hetzner-clippy behavioral gates are GREEN; Linux no-regression is proven by base-vs-HEAD equivalence. The only shortfall is the literal aggregate count, blocked by box disk (infra), not by the delta.

## Issues Encountered
- One transient Hetzner ssh connect timeout on the first clippy attempt (box reachable on immediate retry — `uptime`/`nproc` confirmed).

## User Setup Required
None.

## Next Phase Readiness
- The third-repaired successor candidate is authored on `plan/f20-unified-audit-repair` at `92cac8bb` (tree `0fd3368b`), ready for **20-44** to (a) resync + commit Cargo.lock authoritatively (the dunce edge; STOP for a package-legitimacy checkpoint only if a `--locked` build reveals a genuinely new crate — none is expected) and (b) re-run the aggregate on a box with adequate free disk to obtain the clean `11509/0/48` seal receipt.
- All Windows compile + real-hardware behavior (#A env-transport, #B cmd quoting, containment counting, lib-test compile) is deferred to the **20-46** msvc re-dispatch.
- **No Phase 20 requirement is completed here** (that is 20-49's job).

## Self-Check: PASSED

- `20-43-SUMMARY.md` present.
- All four task commits present in history: `76da033f`, `c9258219`, `230e4521`, `92cac8bb`.
- Diff vs base `4ca0a8f7` is exactly the 8 declared files (no scope drift).
- macOS check exit 0; Hetzner clippy exit 0; aggregate non-regression proven at base.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
