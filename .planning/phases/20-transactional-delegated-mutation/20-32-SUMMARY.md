---
phase: 20-transactional-delegated-mutation
plan: "32"
subsystem: native-uat
tags: [native-uat, f20, windows-msvc, macos-ephemeral, sealed-repaired-successor]
status: incomplete
disposition: INCOMPLETE/RED
requires:
  - phase: 20-30
    provides: "Sealed repaired-successor SHA/tree (Linux 11509/0 + all-features build)"
  - phase: 20-31
    provides: "Zero-finding cross-audit of the sealed successor (native deferred)"
provides:
  - "Retained candidate-bound native RED evidence for the sealed repaired-successor 17412cf2 (both platforms RED; zero final acceptance markers; NEW distinct RED causes)"
affects: [20-33, 20-35]

key-links:
  - from: .planning/phases/20-transactional-delegated-mutation/20-30-SUMMARY.md
    to: .planning/phases/20-transactional-delegated-mutation/20-32-SUMMARY.md
    via: "sealed successor SHA published to a UAT ref + dispatch branch, dispatched to native runners"
    pattern: "source_sha"

sealed_successor:
  source_sha: 17412cf2f6a8be9d2ec7272f6693f998db4ba2e5
  tree: 00e41519ac6782b05e610fcf7fafc772d5040a5d
evidence:
  uat_ref: refs/f20-native-uat/17412cf2f6a8be9d2ec7272f6693f998db4ba2e5
  uat_ref_state: retained
  uat_ref_deletion: not_authorized
  dispatch_branch: f20-native-uat-17412cf2f6a8be9d2ec7272f6693f998db4ba2e5
  run_id: 30061923589
  run_url: https://github.com/FerroxLabs/wayland-core/actions/runs/30061923589
  request_nonce: 3fa47aa9455c47edf94e08fea963ab36394b10fd

requirements-completed: []

duration: ~35min
completed: 2026-07-24
---

# Phase 20 Plan 32: Native-Proof Re-Dispatch of the Sealed Repaired-Successor — Summary

**One-liner:** One FRESH Sean-authorized native-proof dispatch of the exact sealed repaired-successor `17412cf2` ran on the correct runners (Windows self-hosted msvc `SEANDESKTOP`; macOS ephemeral `f20-macos-ephemeral-Seans-MacBook-Pro`). Both 20-25 RED causes are FIXED (macOS `wcore-sandbox` compiled — no E0599; Windows setup cleared `safe.directory` and reached the proof), but **both platforms came back RED on NEW distinct causes** — Windows on a genuine `live_fs_acl` behavioral defect, macOS on a runner-environment gap (missing `alpine:3.19` image). **No final acceptance markers were emitted; no Phase-20 requirement is completed.**

## Disposition

**INCOMPLETE / RED.** A further repaired successor and a fresh Sean native-proof-dispatch authorization are required. The dispatch was NOT retried (single-attempt invariant honored). The UAT evidence ref is retained (deletion not authorized).

## Sealed successor (verified, unchanged)

- `source_sha = 17412cf2f6a8be9d2ec7272f6693f998db4ba2e5` — commit present in the ferrox clone AND published to `FerroxLabs/wayland-core`.
- `tree = 00e41519ac6782b05e610fcf7fafc772d5040a5d` — verified `17412cf2^{tree} == 00e41519…` before any mutation (locally and on the remote commit object).
- 20-30 sealed it (Linux 11509/0 + all-features build); 20-31 cross-audit CLEAN with native deferred.
- This is `95c81ec6` + the 20-29 fix. Both 20-29 fixes were statically confirmed **present in the sealed tree itself** (no Mac cargo run) before spending the scarce run:
  - `safe.directory '*'` guard present on both candidate jobs (`nightly-windows-soak.yml:232` Windows, `:281` macOS).
  - `rename_into` now DEFINED at `crates/wcore-sandbox/src/directory_authority.rs:655` — the exact `E0599` that RED'd macOS at 20-25 (`directory_authority_tests.rs:149`).

## What was executed (single native-proof dispatch)

1. **Publish (one attempt, both refs absent → created):** pushed the sealed SHA to `refs/f20-native-uat/17412cf2…` (retained UAT evidence ref) AND created the dispatch branch `f20-native-uat-17412cf2…` at the same sealed SHA on `FerroxLabs/wayland-core`. Both remote refs verified pointing at `17412cf2…`; remote commit tree verified `00e41519…`.
2. **Persist request tuple:** `node scripts/f20-native-uat-proof.mjs request …` wrote the Git-private pending request `{candidate/commit=17412cf2…, tree=00e41519…, ref, runner_label=f20-native-macos, image_label=f20-image-2e61537b…, nonce=3fa47aa9…}` (nonce = `openssl rand -hex 20`, 40 hex). Byte-identical idempotent echo confirmed.
3. **Dispatch once (on the BRANCH):** `gh workflow run nightly-windows-soak.yml --ref f20-native-uat-17412cf2… -f f20_candidate=true -f f20_macos_runner_label=f20-image-2e61537b… -f f20_request_nonce=3fa47aa9…` → run `30061923589`. `workflow_dispatch --ref` on the branch (never the `refs/f20-native-uat/*` namespace, which returns HTTP 422) so `github.sha == 17412cf2…`.
4. **Reconcile (single-attempt):** run `30061923589` created `2026-07-24T02:33:09Z` > dispatch boundary `2026-07-24T02:32:35Z`, headSha `17412cf2…`, headBranch `f20-native-uat-17412cf2…`, event `workflow_dispatch`, NOT in the pre-existing run set (highest pre-existing `30054999992`). Exactly one dispatch run on the sealed SHA. Hosted `windows-2022` soak correctly `skipped` in candidate mode.

## Per-target native results

### Windows — job `89385115575`, runner `SEANDESKTOP` (self-hosted msvc) — RED (genuine candidate defect)

- **Landed on the correct runner** (self-hosted msvc, NOT hosted `windows-2022`).
- **20-25 infra block is FIXED.** Setup, "Trust the candidate checkout for git" (`safe.directory '*'`), Rust toolchain, and cargo-nextest install all PASSED; the git dubious-ownership abort that stopped 20-25 before the proof did NOT recur. The proof step ran.
- **Failed on target 1 `windows-retained-handle`** — `wcore-sandbox::live_fs_acl::one_execution_grant_never_leaks_to_another_identity` panicked at `crates\wcore-sandbox\tests\live_fs_acl.rs:382:5`:
  ```
  assertion `left == right` failed: the granting identity's read must succeed (exit 0), not choice's selection index
  ```
  nextest retried (TRY 1 + TRY 2), both FAIL. `native Windows target windows-retained-handle failed with exit code 100`.
- **Cause: a genuine Windows behavioral defect in the sealed candidate.** The granting identity's read did not return exit 0 — it returned `choice.exe`'s selection index instead. The 20-20 `choice.exe` exit-code handling does NOT hold on real Windows hardware. This is candidate source debt, not an environment gap — it requires a source fix and re-seal.
- Windows target PASS markers: **none.** Final `F20_NATIVE_WINDOWS_ACCEPTANCE`: **absent.**

### macOS — job `89385115626`, runner `f20-macos-ephemeral-Seans-MacBook-Pro` (ephemeral, machine `Seans-MacBook-Pro`) — RED (runner-environment gap)

- **Landed on the correct ephemeral runner**, which **deregistered after its one job** (ran on this Mac, then self-deregistered per its single-job lifecycle).
- **Both fixed 20-25 causes HELD on hardware:**
  - `wcore-sandbox` COMPILED — NO `E0599`; the `rename_into` fix resolved.
  - `macos-retained-directory` (the rename acceptance, the exact 20-25 macOS RED cause) emitted a PASS marker — handle-relative, decoy-immune rename acceptance is green on hardware.
- **3 ordered target PASS markers emitted** (all bound to `commit=17412cf2… tree=00e41519… nonce=3fa47aa9…`):
  1. `macos-retained-directory` — PASS
  2. `macos-process-tree` — PASS
  3. `macos-docker-reject-path-replacement` — PASS
- **Failed on target 4 `macos-docker-roundtrip-delete`** — `docker_smoke::docker_runs_hello_world` panicked:
  ```
  called `Result::unwrap()` on an `Err` value:
    DockerIo("Docker responded with status code 404: No such image: alpine:3.19")
  → native macOS target macos-docker-roundtrip-delete failed
  ```
- **Cause: a runner-environment gap, NOT candidate code.** The `alpine:3.19` test image was not present on the ephemeral runner and the proof does not pre-pull it. The candidate's own fixed causes passed; the macOS RED is an infra provisioning gap (pre-pull `alpine:3.19` on the runner, or have the proof pull it, before re-dispatch).
- macOS targets 4–8 (`macos-docker-roundtrip-delete`, `macos-public-dispatch`, `macos-docker-cancellation`, `macos-docker-budget`, `macos-f20-lifecycle`): **not reached.** Final `F20_NATIVE_MACOS_ACCEPTANCE`: **absent.**

## Runner identities (recorded)

| Platform | Job | Runner | Machine | Correct? | Post-run |
|----------|-----|--------|---------|----------|----------|
| Windows | 89385115575 | SEANDESKTOP (self-hosted, Windows, X64, msvc) | SEANDESKTOP | Yes (AppContainer-capable self-hosted msvc) | still online |
| macOS | 89385115626 | f20-macos-ephemeral-Seans-MacBook-Pro (ephemeral, image label f20-image-2e61537b…) | Seans-MacBook-Pro | Yes (freshly-registered ephemeral) | deregistered (one-job) |
| Windows soak | 89385115931 | — | — | Correctly `skipped` (candidate mode) | n/a |

## Retained-ref recheck

`git ls-remote https://github.com/FerroxLabs/wayland-core.git refs/f20-native-uat/17412cf2…` → `17412cf2f6a8be9d2ec7272f6693f998db4ba2e5` after capture. **Retained; deletion not authorized.** The dispatch branch `f20-native-uat-17412cf2…` (same sealed SHA) is left in place; not deleted (no cleanup authorization).

## Fixed-cause vs new-cause ledger

| 20-25 RED cause | This run (17412cf2) |
|------------------|---------------------|
| macOS `wcore-sandbox` `E0599` (rename_into missing) | **FIXED** — compiled; `macos-retained-directory` PASS |
| Windows `safe.directory` setup abort before proof | **FIXED** — setup cleared; proof ran |
| — (new) Windows `live_fs_acl` exit-0 defect | **NEW RED** — genuine candidate defect |
| — (new) macOS `alpine:3.19` image absent | **NEW RED** — runner-environment gap |

## Explicit non-claims

- **No Phase-20 requirement is completed here** (the aggregate is 20-35). REQ-native-r14/r4/r7/r9/r11/r3 remain unproven.
- Native PASS is **not** claimed for either platform. No cross-compilation or source-inspection substitute for the missing hardware proof was used. The macOS partial (3/8 ordered target markers) is NOT an acceptance — no `F20_NATIVE_MACOS_ACCEPTANCE` marker exists.

## 20-33 admission decision

**20-33 (fresh review) is NOT admitted.** A further repaired successor + fresh authorization is required, because:

1. **Windows is a genuine candidate defect (blocking):** the sealed successor `17412cf2` fails `live_fs_acl::one_execution_grant_never_leaks_to_another_identity` on real Windows — the granting identity's read does not return exit 0 (returns `choice.exe`'s selection index). This must be fixed in source, re-sealed, and re-authorized.
2. **macOS is a runner-environment gap (blocking a green macOS run, not candidate code):** the ephemeral runner lacks `alpine:3.19`; the `docker_smoke` targets cannot pass until the image is pre-pulled (or the proof pulls it). The candidate's own fixed causes passed on hardware.

**Required next step:** a further repaired successor plan that (a) fixes the Windows `live_fs_acl` exit-0 handling, (b) provisions `alpine:3.19` on the ephemeral macOS runner (or has the proof pull it), (c) re-seals the candidate, and (d) obtains fresh Sean native-proof-dispatch authorization for the new sealed digest.

## Self-Check

- Sealed commit/tree verified before mutation: PASS (`17412cf2^{tree} == 00e41519…`, local + remote).
- Single dispatch invariant: PASS (exactly one `workflow_dispatch` run `30061923589` on the sealed SHA, post-boundary, not pre-existing; no retry after RED).
- Correct runners: PASS (Windows self-hosted msvc `SEANDESKTOP`; macOS ephemeral `f20-macos-ephemeral-Seans-MacBook-Pro`, deregistered).
- Retained ref: PASS (`refs/f20-native-uat/17412cf2…` → sealed SHA post-capture).
- Honesty gate: PASS (both platforms RED, zero final acceptance markers, no green faked, no requirement claimed; Windows-defect vs macOS-env-gap distinguished).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
