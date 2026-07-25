---
phase: 20-transactional-delegated-mutation
plan: "30"
subsystem: build
tags: [native-repair, candidate-seal, cargo-lock, locked-build, aggregate-nextest, ci-profile, no-regression, hetzner-proof, remote-cargo]

# Dependency graph
requires:
  - phase: "20-29"
    provides: "Repaired-successor candidate source_sha 17412cf2 (tree 00e41519) over the sealed 95c81ec6 — the two RED-cause fixes (unix rename_into + safe.directory guard) and nothing else"
provides:
  - "The sealed repaired-successor candidate SHA (17412cf2, tree 00e41519) carrying a consistent, UNCHANGED Cargo.lock (REQ-native-r10)"
  - "A Hetzner --locked --workspace --all-features build receipt proving the committed lock is consistent (cargo did not need to update it under --locked)"
  - "A Hetzner aggregate nextest --profile ci receipt at 11509 passed / 0 failed / 48 skipped on the authoritative remote-cargo land-gate harness — no Linux regression from the 20-29 rename_into unification + workflow fix (REQ-native-r4)"
affects: ["20-31", "20-32", "20-33", "20-34", "20-35"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Zero-delta re-seal: the 20-29 fix touched no Cargo.toml and added no dependency, so a --locked Hetzner build succeeds with no lock update and NO sealing commit is created — the sealed candidate is the source-complete 20-29 HEAD itself"
    - "Receipt binds the EXACT sealed tree: proofs run against a detached checkout of 17412cf2 so the remote-cargo harness verifies the shipped tree hash == 00e41519 (the sealed candidate tree), not a docs-superset tree"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-30-SUMMARY.md
  modified: []

key-decisions:
  - "No Cargo.lock commit was created: the --locked --workspace --all-features Hetzner build finished in 1m27s with no 'lock file needs to be updated' message, proving the committed lock (blob 2b8c6cdf, sha256 bbbd8e72…, 1015 [[package]] stanzas — byte-identical to the 20-23 seal) is already consistent with the current manifest surface. The 20-29 delta added zero dependencies (135eb7b1 = 2 sandbox source files; 17412cf2 = 1 workflow YAML; no Cargo.toml touched), so no delta was possible and none appeared. Per the execution rules, with zero delta the sealed candidate is the 20-29 source-complete HEAD 17412cf2, not a new lock commit."
  - "Proved against a detached checkout of the exact sealed candidate 17412cf2 (tree 00e41519) rather than the branch tip 346c4ead. 346c4ead is the doc-only 20-29 SUMMARY commit sitting on top of 17412cf2; git diff 17412cf2..346c4ead is exactly the 154-line 20-29 SUMMARY in .planning/ and nothing else, so the buildable surface (crates/, Cargo.toml, Cargo.lock) is byte-identical. Detaching makes the remote-cargo tree-hash gate verify 00e41519 exactly, so every receipt binds the precise sealed tree downstream gates bind to."
  - "Reused a single remote-cargo slot (f20-30-aggregate) for all three commands (locked build → plain build → aggregate) so the warm target is shared, mirroring the 20-23 single-slot approach. The plan's per-command slot names (f20-30-locked-build / f20-30-build / f20-30-aggregate) are cache identities, not correctness inputs; one slot yields identical receipts at lower cost."

requirements-completed: []  # NO Phase-20 requirement is claimed here. r10/r4 have their build/aggregate evidence but native-proof + terminal requirement completion remain deferred to 20-32/20-35.

# Coverage metadata
coverage:
  - id: D1
    description: "Cargo.lock is confirmed consistent on the sealed successor — a --locked all-features Hetzner build succeeds without updating it; no new external crate; no lock commit needed"
    requirement: "REQ-native-r10"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-30-aggregate <ferrox-clone> build --workspace --all-features --locked (detached HEAD 17412cf2, tree 00e41519) -> EXIT=0, Finished dev profile in 1m27s, NO 'lock file needs to be updated' line (proves committed lock consistent under --locked). Lock blob 2b8c6cdf, sha256 bbbd8e72e5b63a402c8ae72fb682bc9ad24d901317324370acc08fe72f99aaa5, 1015 [[package]] stanzas — byte-identical to the 20-23 seal; zero delta."
        status: pass
    human_judgment: false
  - id: D2
    description: "The sealed successor builds all-features and the aggregate Linux suite is unregressed at the exact reviewed baseline"
    requirement: "REQ-native-r4"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-30-aggregate <ferrox-clone> build --workspace --all-features (detached 17412cf2) -> EXIT=0, Finished in 8.14s (warm slot)"
        status: pass
      - kind: e2e
        ref: "remote-cargo.sh f20-30-aggregate <ferrox-clone> nextest run --workspace --profile ci --no-fail-fast (detached 17412cf2, tree 00e41519 verified) -> EXIT=0, Nextest run ID 961262a4-8133-471b-866b-43fbc5c662f0, Summary [70.669s] 11509 tests run: 11509 passed (2 flaky), 48 skipped == 11509/0/48, the exact prior-seal baseline. Spot-checks PASS: transactional_delegated_mutation_test (9/9), anvil_forge_transaction drive_climb_full_lands_the_winner_surface_for_accept [1.287s], packaged_f04_run_is_repeatable_and_content_addressed [55.153s] (under the 90s ci kill), fix1_dispatch_budget_aborts_with_partial_result [68.830s] (requires the ci profile's 90s+2-retries)."
        status: pass
    human_judgment: false

# Metrics
duration: ~15min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 30: Re-seal the Repaired-Successor Candidate — Cargo.lock Consistency + Hetzner Build & Aggregate Proof Summary

**Re-sealed the ONE repaired-successor candidate `17412cf2` (tree `00e41519`): the 20-29 fix added zero dependencies, so a `--locked --workspace --all-features` Hetzner build succeeds with NO lock update and NO sealing commit — the sealed candidate IS the 20-29 source-complete HEAD. Proved on the authoritative remote-cargo land-gate harness (against the exact detached tree `00e41519`) that the successor builds all-features (EXIT=0) and passes the aggregate `nextest --profile ci` at exactly 11509 passed / 0 failed / 48 skipped — no Linux regression from the 20-29 `rename_into` unification + `safe.directory` workflow fix. No source, test, or workflow file changed; no Phase-20 requirement completed.**

## Candidate Tuple

- **Sealed source_sha:** `17412cf2f6a8be9d2ec7272f6693f998db4ba2e5` — tree `00e41519ac6782b05e610fcf7fafc772d5040a5d`
- **Predecessor (sealed 20-23/20-25 candidate):** `95c81ec6a351ec22125497333739fa7c93a0cd8b` — tree `784f498002b9944856aedee6cb3db347b55c1dcc`
- **Branch tip at seal time:** `346c4eadd7000e124141acf474884b0ac4cc80ff` — the doc-only `docs(20-29)` SUMMARY commit sitting on top of the sealed candidate. `git diff 17412cf2..346c4ead` = exactly the 154-line 20-29 SUMMARY in `.planning/`; the buildable surface (`crates/`, `Cargo.toml`, `Cargo.lock`) is byte-identical.
- **task_base (scope authority):** captured at `346c4ead`, generation `g-eefad52156dc4a92d81b65c0221aa1b7ea64d642c7f3fe88cce024137f3335cd`
- **Touched paths this plan:** NONE (no source, no test, no workflow, no lockfile). Zero lock delta → no sealing commit.
- **This is the sealed candidate.** After this seal, NO source, test, or workflow file changes for the remainder of the repaired-successor sequence; every downstream gate (20-31 cross-audit, 20-32 native re-dispatch, 20-33 review, 20-34 prep, 20-35 terminal) binds to `17412cf2`.

## The Lock Delta

**None.** The committed `Cargo.lock` at the sealed candidate is byte-identical to the 20-23 seal:

- Last-touch: `95c81ec6` (the 20-23 candidate seal). Nothing has touched it since.
- Blob at HEAD: `2b8c6cdfedb84b0a52be2c5292805c83e377a2b8` — sha256 `bbbd8e72e5b63a402c8ae72fb682bc9ad24d901317324370acc08fe72f99aaa5` — 1015 `[[package]]` stanzas. (Identical to the sha256 recorded in the 20-23 summary.)
- The 20-29 delta added **zero** dependencies: `135eb7b1` touched only two `wcore-sandbox` source files (reusing existing `libc`/`renameat` machinery); `17412cf2` touched only `.github/workflows/nightly-windows-soak.yml`. No `Cargo.toml` was touched, so no new lock edge or crate was possible.
- The `--locked --workspace --all-features` Hetzner build finished cleanly with **no** "the lock file … needs to be updated but --locked was passed" message → the committed lock is fully consistent with the manifest surface. **No `generate-lockfile`/`metadata` regeneration was required, and no lock commit was created.**
- **No new external crate** was introduced → no package-legitimacy checkpoint required (per execution rules).

## Hetzner Receipts (authoritative: remote-cargo land-gate harness, exact sealed tree `00e41519`)

The remote-cargo harness materializes the committed tree in a clean per-invocation slot and verifies the extracted tree hash equals the local HEAD tree before cargo runs. All three proofs ran against a **detached checkout of `17412cf2`** (worktree clean), so the harness verified the shipped tree == `00e41519` — the exact sealed candidate tree. A single slot (`f20-30-aggregate`) was reused across all three commands to share the warm target.

### Receipt 1 — locked build (Task 1 verify / REQ-native-r10)
- **Command:** `remote-cargo.sh f20-30-aggregate <ferrox-clone> build --workspace --all-features --locked`
- **Exit:** `0`. **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 27s`.
- **Lock consistency:** no "lock file … needs to be updated but --locked was passed" — `--locked` succeeded, proving the committed lock is consistent (cargo did not need to change it).
- **Only warning:** a pre-existing, unrelated future-incompat note for `imap-proto v0.10.2` (not a failure; predates this candidate).

### Receipt 2 — plain all-features build (Task 2 verify / REQ-native-r4)
- **Command:** `remote-cargo.sh f20-30-aggregate <ferrox-clone> build --workspace --all-features`
- **Exit:** `0`. **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 8.14s` (warm slot).

### Receipt 3 — aggregate nextest (Task 2 verify / REQ-native-r4)
- **Command:** `remote-cargo.sh f20-30-aggregate <ferrox-clone> nextest run --workspace --profile ci --no-fail-fast`
- **Exit:** `0`. **Nextest run ID:** `961262a4-8133-471b-866b-43fbc5c662f0` (profile: ci; 469 binaries; `Starting 11509 tests … (48 tests skipped)`).
- **Result:** `Summary [ 70.669s] 11509 tests run: 11509 passed (2 flaky), 48 skipped` — **11509 / 0 / 48**, the exact reviewed baseline (identical to the 20-23 seal count).
- **Spot-checks (all PASS):** `wcore-agent::transactional_delegated_mutation_test` 9/9 PASS; `wcore-agent::anvil_forge_transaction production_landing::drive_climb_full_lands_the_winner_surface_for_accept` PASS [1.287s]; `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` PASS [55.153s] (under the 90s ci kill — the reason the ci profile is used); `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` PASS [68.830s] (the test that flakes only under the default profile; the ci profile's 90s timeout + 2 retries passes it).
- **2 flaky (passed on retry, counted as passed):** `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` (2/3) and `wcore-swarm::swarm_worker_failure_reporting_e2e swarm_reports_failed_worker_status_and_succeeding_workers_complete` (2/3) — normal retry behavior for these subprocess-spawning tests under the ci profile.
- **Real failures:** NONE (0 failed / 0 timed out).

## Honesty Gate

The aggregate is exactly `11509/0/48` — the same reviewed baseline as the prior seal — so the honesty gate did not trip. The `fix1_dispatch_budget_aborts_with_partial_result` test that flakes under the default profile PASSED under the ci profile as designed ([68.830s], within the 90s ci kill). No count was faked, no discrepancy was rationalized, and no STOP was required.

## No Linux Regression — Candidate Independence

The 20-29 delta changes nothing Linux-compiled at runtime: the macOS acceptance test is `#[cfg(target_os = "macos")]` (adds nothing to the Linux count), the unix `rename_into` unification of `atomic_write_child`'s publish is behavior-preserving over the existing atomic-write tests, and the `safe.directory` workflow guard is CI-only. The lock is unchanged (same resolved versions). The aggregate matching the exact `11509/0/48` baseline confirms no regression from the `rename_into` unification or the workflow fix.

## Deviations from Plan

- **[No-delta path — plan-anticipated] No `Cargo.lock` commit and no `verify-task-scope.sh` scope-gate run.** The plan's Task 1 verify block (`verify-task-scope.sh <base> Cargo.lock`) and the `files_modified: [Cargo.lock]` frontmatter assume a lock change would be produced. As the plan itself anticipates ("If it succeeds without updating the lock (expected — the fix added no dependency), record the 20-29 source-complete HEAD SHA/tree as the sealed successor candidate and make no lock commit"), the expected zero-delta outcome occurred. The scope gate requires its `required-path` (`Cargo.lock`) to appear in the observed change set and would fail with "required path absent" when nothing changed — so it is inapplicable to the zero-delta path. Scope was instead demonstrated directly: the task base was captured at `346c4ead`, and `git diff 346c4ead..HEAD` plus `git status --short` are both empty, proving no file was touched (trivially in-scope). No source impact.
- **[Slot reuse] Single remote-cargo slot for all three commands.** The plan names `f20-30-locked-build` / `f20-30-build` / `f20-30-aggregate`; one slot (`f20-30-aggregate`) was reused for all three so the warm target is shared, mirroring the 20-23 single-slot approach. Slot names are cache identities, not correctness inputs — the receipts are identical.
- **[Exact-tree proof] Proofs run against detached `17412cf2`, not the branch tip.** Ran against a detached checkout of the exact sealed candidate so the remote-cargo tree-hash gate verifies `00e41519` precisely, rather than the docs-superset tree at `346c4ead`. The buildable surface is byte-identical; this only tightens the receipt binding.

## Explicit Non-Claims / Deferrals

- **No Phase-20 requirement is marked complete.** REQ-native-r10 and REQ-native-r4 have their lock-consistency + build + aggregate evidence here, but native-proof and requirement completion remain deferred (r10/r4 completion and the terminal claim are 20-32/20-35 per the sequence).
- **No macOS/Windows native RUNTIME claim** is made — this plan proves only the Linux `--locked` build + aggregate on the sealed candidate. The decoy-planted macOS acceptance test and the self-hosted Windows candidate run are deferred to the 20-32 native re-dispatch with fresh Sean authorization.
- **After this seal, NO source, test, or workflow change** follows for the remainder of the repaired-successor sequence; `17412cf2` (tree `00e41519`) is the exact candidate all downstream gates (20-31…20-35) bind to.
- No push, no merge; work is on `plan/f20-unified-audit-repair` in the ferrox clone only.

## Self-Check: PASSED

- Sealed candidate `17412cf2` (tree `00e41519`) — FOUND in `git log` on `plan/f20-unified-audit-repair`; source-complete 20-29 HEAD, one doc-only commit below the branch tip.
- `Cargo.lock` — UNCHANGED; blob `2b8c6cdf`, sha256 `bbbd8e72…`, 1015 packages; last-touch `95c81ec6`; zero delta this plan.
- Receipt 1 (remote-cargo `--locked` all-features build): EXIT=0, `Finished` in 1m27s, no lock-update — lock consistent.
- Receipt 2 (remote-cargo plain all-features build): EXIT=0, `Finished` in 8.14s.
- Receipt 3 (remote-cargo aggregate `nextest --profile ci`): EXIT=0, run ID `961262a4-8133-471b-866b-43fbc5c662f0`, `11509 passed / 0 failed / 48 skipped`.
- Scope: task base captured at `346c4ead`; `git diff 346c4ead..HEAD` and `git status --short` both empty — no file touched.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
*Candidate re-sealed at `17412cf2` (tree `00e41519`); Cargo.lock unchanged; no source/test/workflow changes follow; native/requirement completion deferred to 20-32/20-35.*
