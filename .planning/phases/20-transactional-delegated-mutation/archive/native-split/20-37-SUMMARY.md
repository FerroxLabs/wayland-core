---
phase: 20-transactional-delegated-mutation
plan: "37"
subsystem: build
tags: [native-repair, candidate-seal, cargo-lock, locked-build, aggregate-nextest, ci-profile, no-regression, hetzner-proof, remote-cargo, second-repaired-successor]

# Dependency graph
requires:
  - phase: "20-36"
    provides: "Second repaired-successor candidate source_sha daf27337 (tree 91de96a3) over the sealed 17412cf2 — the two NEW 20-32 RED-cause fixes (Windows `exit /b 0` exit-0 fidelity + macOS docker-image pre-pull) and nothing else"
provides:
  - "The sealed second-repaired-successor candidate SHA (daf27337, tree 91de96a3) carrying a consistent, UNCHANGED Cargo.lock (REQ-native-r10)"
  - "A Hetzner --locked --workspace --all-features build receipt proving the committed lock is consistent (cargo did not need to update it under --locked)"
  - "A Hetzner aggregate nextest --profile ci receipt at 11509 passed / 0 failed / 48 skipped on the authoritative remote-cargo land-gate harness — no Linux regression from the 20-36 Windows exit-code + macOS docker-pull fixes (REQ-native-r4)"
affects: ["20-38", "20-39", "20-40", "20-41", "20-42"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Zero-delta re-seal: the 20-36 fix touched no Cargo.toml and added no dependency, so a --locked Hetzner build succeeds with no lock update and NO sealing commit is created — the sealed candidate is the source-complete 20-36 HEAD itself"
    - "Receipt binds the EXACT sealed tree: proofs run against a detached checkout of daf27337 so the remote-cargo harness verifies the shipped tree hash == 91de96a3 (the sealed candidate tree), not a docs-superset tree"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-37-SUMMARY.md
  modified: []

key-decisions:
  - "No Cargo.lock commit was created: the --locked --workspace --all-features Hetzner build finished in 1m26s with no 'lock file needs to be updated' message, proving the committed lock (blob 2b8c6cdf, sha256 bbbd8e72…, 1015 [[package]] stanzas — byte-identical to the 20-23/20-30 seal) is already consistent with the current manifest surface. The 20-36 delta added zero dependencies (edits were two `#![cfg(windows)]` test-helper files and one bash proof-script; no Cargo.toml touched), so no delta was possible and none appeared. Per the execution rules, with zero delta the sealed candidate is the 20-36 source-complete HEAD daf27337, not a new lock commit."
  - "Proved against a detached checkout of the exact sealed candidate daf27337 (tree 91de96a3) rather than the branch tip fd486d05. fd486d05 is the doc-only 20-36 SUMMARY commit sitting on top of daf27337; git diff daf27337..fd486d05 is exactly the 20-36 SUMMARY in .planning/ and nothing else, so the buildable surface (crates/, Cargo.toml, Cargo.lock) is byte-identical. Detaching makes the remote-cargo tree-hash gate verify 91de96a3 exactly, so every receipt binds the precise sealed tree downstream gates bind to."
  - "Reused a single remote-cargo slot (f20-37-aggregate) for all three commands (locked build → plain build → aggregate) so the warm target is shared, mirroring the 20-30 single-slot approach. The plan's per-command slot names are cache identities, not correctness inputs; one slot yields identical receipts at lower cost."

requirements-completed: []  # NO Phase-20 requirement is claimed here. r10/r4 have their build/aggregate evidence but native-proof + terminal requirement completion remain deferred to 20-39/20-42.

# Coverage metadata
coverage:
  - id: D1
    description: "Cargo.lock is confirmed consistent on the sealed second-repaired successor — a --locked all-features Hetzner build succeeds without updating it; no new external crate; no lock commit needed"
    requirement: "REQ-native-r10"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-37-aggregate <ferrox-clone> build --workspace --all-features --locked (detached HEAD daf27337, tree 91de96a3) -> EXIT=0, Finished dev profile in 1m26s, NO 'lock file needs to be updated' line (proves committed lock consistent under --locked). Lock blob 2b8c6cdf, sha256 bbbd8e72e5b63a402c8ae72fb682bc9ad24d901317324370acc08fe72f99aaa5, 1015 [[package]] stanzas — byte-identical to the 20-23/20-30 seal; zero delta; last-touch 95c81ec6."
        status: pass
    human_judgment: false
  - id: D2
    description: "The sealed second-repaired successor builds all-features and the aggregate Linux suite is unregressed at the exact reviewed baseline"
    requirement: "REQ-native-r4"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-37-aggregate <ferrox-clone> build --workspace --all-features (detached daf27337) -> EXIT=0, Finished in 8.08s (warm slot)"
        status: pass
      - kind: e2e
        ref: "remote-cargo.sh f20-37-aggregate <ferrox-clone> nextest run --workspace --profile ci --no-fail-fast (detached daf27337, tree 91de96a3 verified) -> EXIT=0, Nextest run ID a2e2c59c-5ef4-40c4-9672-57c477f143fb, Summary [70.698s] 11509 tests run: 11509 passed (3 flaky), 48 skipped == 11509/0/48, the exact prior-seal baseline. Spot-checks PASS: transactional_delegated_mutation_test (all PASS), anvil_forge_transaction drive_climb_full_lands_the_winner_surface_for_accept [1.472s], packaged_f04_run_is_repeatable_and_content_addressed [54.731s] (under the 90s ci kill), fix1_dispatch_budget_aborts_with_partial_result [68.879s] (requires the ci profile's 90s+2-retries)."
        status: pass
    human_judgment: false

# Metrics
duration: ~10min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 37: Re-seal the Second Repaired-Successor Candidate — Cargo.lock Consistency + Hetzner Build & Aggregate Proof Summary

**Re-sealed the ONE second-repaired-successor candidate `daf27337` (tree `91de96a3`): the 20-36 fix (Windows `exit /b 0` exit-0 fidelity + macOS docker-image pre-pull) added zero dependencies, so a `--locked --workspace --all-features` Hetzner build succeeds with NO lock update and NO sealing commit — the sealed candidate IS the 20-36 source-complete HEAD. Proved on the authoritative remote-cargo land-gate harness (against the exact detached tree `91de96a3`) that the successor builds all-features (EXIT=0) and passes the aggregate `nextest --profile ci` at exactly 11509 passed / 0 failed / 48 skipped — no Linux regression from the 20-36 edits (the Windows helpers are `#![cfg(windows)]` = zero Linux tests; the macOS proof harness is a shell script, not Linux-compiled). No source, test, or workflow file changed; no Phase-20 requirement completed.**

## Candidate Tuple

- **Sealed source_sha:** `daf273373eddcb22a94e26988747e6f74ff81bde` — tree `91de96a3f703423dac23408f20b397b6fbfeee00`
- **Predecessor (sealed 20-30 candidate):** `17412cf2f6a8be9d2ec7272f6693f998db4ba2e5` — tree `00e41519ac6782b05e610fcf7fafc772d5040a5d`
- **Branch tip at seal time:** `fd486d056a5085355b82490dcad7390db36cb94c` — the doc-only `docs(20-36)` SUMMARY commit sitting on top of the sealed candidate. `git diff daf27337..fd486d05` = exactly the 20-36 SUMMARY in `.planning/`; the buildable surface (`crates/`, `Cargo.toml`, `Cargo.lock`) is byte-identical.
- **task_base (scope authority):** captured at `fd486d05`, generation `g-f5fb02178ee5925f148b0b9d410d8375834469d55fc9dd043169a577506f03d8`
- **Touched paths this plan:** NONE (no source, no test, no workflow, no lockfile). Zero lock delta → no sealing commit.
- **This is the sealed candidate.** After this seal, NO source, test, or workflow file changes for the remainder of the second-repaired-successor sequence; every downstream gate (20-38 pre-native cross-audit, 20-39 native re-dispatch, 20-40 fresh review, 20-41 re-prep tuple, 20-42 terminal) binds to `daf27337`.

## The Lock Delta

**None.** The committed `Cargo.lock` at the sealed candidate is byte-identical to the 20-23/20-30 seal:

- Last-touch: `95c81ec6` (the 20-23 candidate seal, `chore(f20-23): seal candidate — resync Cargo.lock to declared deps`). Nothing has touched it since.
- Blob at the sealed candidate: `2b8c6cdfedb84b0a52be2c5292805c83e377a2b8` — sha256 `bbbd8e72e5b63a402c8ae72fb682bc9ad24d901317324370acc08fe72f99aaa5` — 1015 `[[package]]` stanzas. (Identical to the sha256 recorded in the 20-23 and 20-30 summaries; blob at daf27337 and at branch tip fd486d05 are the same object.)
- The 20-36 delta added **zero** dependencies: the three touched files were `crates/wcore-sandbox/tests/live_fs_acl.rs`, `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (both `#![cfg(windows)]` test helpers whose exit-code tails changed), and `scripts/f20-native-macos-proof.sh` (a bash proof script). No `Cargo.toml` was touched, so no new lock edge or crate was possible.
- The `--locked --workspace --all-features` Hetzner build finished cleanly with **no** "the lock file … needs to be updated but --locked was passed" message → the committed lock is fully consistent with the manifest surface. **No `generate-lockfile`/`metadata` regeneration was required, and no lock commit was created.**
- **No new external crate** was introduced → no package-legitimacy checkpoint required (per execution rules).

## Hetzner Receipts (authoritative: remote-cargo land-gate harness, exact sealed tree `91de96a3`)

The remote-cargo harness materializes the committed tree in a clean per-invocation slot and verifies the extracted tree hash equals the local HEAD tree before cargo runs. All three proofs ran against a **detached checkout of `daf27337`** (worktree clean, no untracked files), so the harness verified the shipped tree == `91de96a3` — the exact sealed candidate tree. A single slot (`f20-37-aggregate`) was reused across all three commands to share the warm target.

### Receipt 1 — locked build (Task 1 verify / REQ-native-r10)
- **Command:** `remote-cargo.sh f20-37-aggregate <ferrox-clone> build --workspace --all-features --locked`
- **Exit:** `0`. **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 26s`.
- **Lock consistency:** no "lock file … needs to be updated but --locked was passed" — `--locked` succeeded, proving the committed lock is consistent (cargo did not need to change it).
- **Only warning:** a pre-existing, unrelated future-incompat note for `imap-proto v0.10.2` (not a failure; predates this candidate, same as the 20-30 seal).

### Receipt 2 — plain all-features build (Task 2 verify / REQ-native-r4)
- **Command:** `remote-cargo.sh f20-37-aggregate <ferrox-clone> build --workspace --all-features`
- **Exit:** `0`. **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 8.08s` (warm slot).

### Receipt 3 — aggregate nextest (Task 2 verify / REQ-native-r4)
- **Command:** `remote-cargo.sh f20-37-aggregate <ferrox-clone> nextest run --workspace --profile ci --no-fail-fast`
- **Exit:** `0`. **Nextest run ID:** `a2e2c59c-5ef4-40c4-9672-57c477f143fb` (profile: ci; 469 binaries; `Starting 11509 tests … (48 tests skipped)`).
- **Result:** `Summary [ 70.698s] 11509 tests run: 11509 passed (3 flaky), 48 skipped` — **11509 / 0 / 48**, the exact reviewed baseline (identical to the 20-23/20-30 seal count).
- **Spot-checks (all PASS):** `wcore-agent::transactional_delegated_mutation_test` all PASS (happy_path_open_accept_land_receipt_then_rollback, land_selected_winner_drives_production_chain_to_landed, multi_candidate_only_winner_lands_loser_is_cleaned, restart_replays_landed_state_from_disk, etc.); `wcore-agent::anvil_forge_transaction production_landing::drive_climb_full_lands_the_winner_surface_for_accept` PASS [1.472s]; `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` PASS [54.731s] (under the 90s ci kill — the reason the ci profile is used); `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` PASS [68.879s] (the test that flakes only under the default profile; the ci profile's 90s timeout + 2 retries passes it).
- **3 flaky (passed on retry, counted as passed):** `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` (2/3), `wcore-swarm::swarm_worker_failure_reporting_e2e swarm_reports_failed_worker_status_and_succeeding_workers_complete` (2/3), and `wcore-swarm::worker_runtime_limits multi_worker_output_exhaustion_fails_without_retaining_buffers` (2/3) — normal retry behavior for these subprocess-spawning tests under the ci profile. (The 20-30 seal recorded 2 flaky; the flaky COUNT is nondeterministic run-to-run and does not change the 11509/0/48 pass total — all flaky tests passed on retry.)
- **Real failures:** NONE (0 failed / 0 timed out / 0 leaked — `grep -cE '^\s+(FAIL|TIMEOUT|LEAK)'` == 0).

## Honesty Gate

The aggregate is exactly `11509/0/48` — the same reviewed baseline as the prior seal — so the honesty gate did not trip. The `fix1_dispatch_budget_aborts_with_partial_result` test that flakes under the default profile PASSED under the ci profile as designed ([68.879s], within the 90s ci kill). No count was faked, no discrepancy was rationalized, and no STOP was required. The only run-to-run difference from the 20-30 receipt is the flaky COUNT (3 vs 2), which does not affect the pass total and is expected variance for subprocess-spawning tests.

## No Linux Regression — Candidate Independence

The 20-36 delta changes nothing Linux-compiled at runtime: both edited test helpers (`live_fs_acl.rs`, `hard_process_containment_windows.rs`) are `#![cfg(windows)]` and compile to zero tests on Linux (so they add nothing to the Linux count and cannot regress it), and the macOS proof harness (`scripts/f20-native-macos-proof.sh`) is a shell script that is never Linux-compiled. The lock is unchanged (same resolved versions). The aggregate matching the exact `11509/0/48` baseline confirms no regression from the Windows exit-code change or the macOS docker-image pre-pull.

## Deviations from Plan

- **[No-delta path — plan-anticipated] No `Cargo.lock` commit and no `verify-task-scope.sh` required-path scope gate.** The plan's Task 1 verify block (`verify-task-scope.sh <base> Cargo.lock`) and the `files_modified: [Cargo.lock]` frontmatter assume a lock change would be produced. As the plan itself anticipates ("If it succeeds without updating the lock (expected — the fix added no dependency), record the 20-36 source-complete HEAD SHA/tree as the sealed successor candidate and make no lock commit"), the expected zero-delta outcome occurred. The scope gate requires its required-path (`Cargo.lock`) to appear in the observed change set and would fail with "required path absent" when nothing changed — so it is inapplicable to the zero-delta path. Scope was instead demonstrated directly: the task base was captured at `fd486d05` (generation `g-f5fb0217…`), and `git diff fd486d05..HEAD` plus `git status --short` are both empty, proving no file was touched (trivially in-scope). No source impact.
- **[Slot reuse] Single remote-cargo slot for all three commands.** One slot (`f20-37-aggregate`) was reused for all three so the warm target is shared, mirroring the 20-30 single-slot approach. Slot names are cache identities, not correctness inputs — the receipts are identical.
- **[Exact-tree proof] Proofs run against detached `daf27337`, not the branch tip.** Ran against a detached checkout of the exact sealed candidate so the remote-cargo tree-hash gate verifies `91de96a3` precisely, rather than the docs-superset tree at `fd486d05`. The buildable surface is byte-identical; this only tightens the receipt binding.

## Explicit Non-Claims / Deferrals

- **No Phase-20 requirement is marked complete.** REQ-native-r10 and REQ-native-r4 have their lock-consistency + build + aggregate evidence here, but native-proof and requirement completion remain deferred (r10/r4 completion and the terminal claim are 20-39/20-42 per the sequence).
- **No macOS/Windows native RUNTIME claim** is made — this plan proves only the Linux `--locked` build + aggregate on the sealed candidate. The Windows exit-code fix lives entirely in `#![cfg(windows)]` files (compiles to nothing on the Mac or Hetzner Linux) and the macOS docker pre-pull is a shell-script change; both are proven only on the self-hosted msvc AppContainer runner and the macOS ephemeral runner at the 20-39 native re-dispatch with fresh Sean authorization.
- **After this seal, NO source, test, or workflow change** follows for the remainder of the second-repaired-successor sequence; `daf27337` (tree `91de96a3`) is the exact candidate all downstream gates (20-38…20-42) bind to.
- No push, no merge; work is on `plan/f20-unified-audit-repair` in the ferrox clone only.

## Self-Check: PASSED

- Sealed candidate `daf27337` (tree `91de96a3`) — FOUND in `git log` on `plan/f20-unified-audit-repair`; source-complete 20-36 HEAD, one doc-only commit below the branch tip `fd486d05`.
- `Cargo.lock` — UNCHANGED; blob `2b8c6cdf`, sha256 `bbbd8e72…`, 1015 packages; last-touch `95c81ec6`; zero delta this plan.
- Receipt 1 (remote-cargo `--locked` all-features build): EXIT=0, `Finished` in 1m26s, no lock-update — lock consistent.
- Receipt 2 (remote-cargo plain all-features build): EXIT=0, `Finished` in 8.08s.
- Receipt 3 (remote-cargo aggregate `nextest --profile ci`): EXIT=0, run ID `a2e2c59c-5ef4-40c4-9672-57c477f143fb`, `11509 passed / 0 failed / 48 skipped`; 0 FAIL/TIMEOUT/LEAK.
- Scope: task base captured at `fd486d05`; `git diff fd486d05..HEAD` and `git status --short` both empty — no file touched.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
*Candidate re-sealed at `daf27337` (tree `91de96a3`); Cargo.lock unchanged; no source/test/workflow changes follow; native/requirement completion deferred to 20-39/20-42.*
