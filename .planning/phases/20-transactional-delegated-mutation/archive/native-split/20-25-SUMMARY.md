---
phase: 20-transactional-delegated-mutation
plan: "25"
subsystem: native-uat
status: incomplete
disposition: INCOMPLETE/RED
tags: [native-uat, f20, windows-msvc, macos-ephemeral, sealed-candidate]
requires:
  - .planning/phases/20-transactional-delegated-mutation/20-23-SUMMARY.md
  - .planning/phases/20-transactional-delegated-mutation/20-24-SUMMARY.md
provides:
  - "Retained candidate-bound native RED evidence for the sealed candidate (both platforms failed; no acceptance markers)"
key-links:
  - from: .planning/phases/20-transactional-delegated-mutation/20-23-SUMMARY.md
    to: .planning/phases/20-transactional-delegated-mutation/20-25-SUMMARY.md
    via: "sealed candidate SHA published to a UAT ref + dispatch branch, dispatched to native runners"
    pattern: "source_sha"
sealed_candidate:
  source_sha: 95c81ec6a351ec22125497333739fa7c93a0cd8b
  tree: 784f498002b9944856aedee6cb3db347b55c1dcc
evidence:
  uat_ref: refs/f20-native-uat/95c81ec6a351ec22125497333739fa7c93a0cd8b
  uat_ref_state: retained
  uat_ref_deletion: not_authorized
  dispatch_branch: f20-native-uat-95c81ec6a351ec22125497333739fa7c93a0cd8b
  run_id: 30054999992
  run_url: https://github.com/FerroxLabs/wayland-core/actions/runs/30054999992
  request_nonce: d1f9d2b1d404de784b1d46e71de5d0722ada4d2f
requirements_completed: []
metrics:
  completed: 2026-07-24
  windows: RED
  macos: RED
---

# Phase 20 Plan 25: Native-Proof Dispatch of the Sealed Candidate — Summary

**One-liner:** One Sean-authorized native-proof dispatch of the exact sealed candidate `95c81ec6` ran on the correct runners (Windows self-hosted msvc `SEANDESKTOP`; macOS ephemeral `f20-macos-ephemeral-Seans-MacBook-Pro`), and **both platforms came back RED** — macOS on a genuine `wcore-sandbox` lib-test compile error in the sealed tree, Windows on a self-hosted-runner git `safe.directory` misconfiguration before the proof script ran. No acceptance markers were emitted; **no Phase-20 requirement is completed**.

## Disposition

**INCOMPLETE / RED.** A repaired successor plan and a fresh Sean native-proof-dispatch authorization are required. The dispatch was NOT retried (single-attempt honored). The evidence ref is retained (deletion not authorized).

## Sealed candidate (verified, unchanged)

- `source_sha = 95c81ec6a351ec22125497333739fa7c93a0cd8b` — commit object present in the ferrox clone.
- `tree = 784f498002b9944856aedee6cb3db347b55c1dcc` — verified `95c81ec6^{tree} == 784f4980…` before any mutation.
- 20-23 sealed it (Linux 11509/0 + all-features build); 20-24 cross-audit CLEAN with native deferred.

## What was executed (single native-proof dispatch)

1. **Publish (one attempt):** pushed the sealed SHA to `refs/f20-native-uat/95c81ec6a351ec22125497333739fa7c93a0cd8b` on `FerroxLabs/wayland-core` (ref was absent; created new). This is the retained UAT evidence ref.
2. **Persist request tuple:** `node scripts/f20-native-uat-proof.mjs request …` wrote the Git-private pending request `{candidate, commit, tree, ref, runner_label=f20-native-macos, image_label=f20-image-2e61537b…, nonce=d1f9d2b1…}` (nonce = `openssl rand -hex 20`, 40 hex).
3. **Dispatch once:** `gh workflow run nightly-windows-soak.yml --ref f20-native-uat-95c81ec6… -f f20_candidate=true -f f20_macos_runner_label=f20-image-2e61537b… -f f20_request_nonce=d1f9d2b1…` → run `30054999992`. `github.sha == 95c81ec6…` confirmed on both jobs.
4. **Capture/verify:** run bound uniquely (headSha = sealed SHA, `workflow_dispatch`, createdAt `2026-07-24T00:02:39Z` > dispatch boundary `2026-07-24T00:02:37Z`, not in the pre-existing run set; exactly one dispatch run on the sealed SHA). Both native jobs completed `failure`; zero `F20_NATIVE_*` markers emitted.

### Harness↔plan mechanics resolved (Rule 3 — blocking fix)

The plan/brief literal `gh workflow run --ref refs/f20-native-uat/<sha>` fails with **HTTP 422 "No ref found"** — `workflow_dispatch` resolves only branches/tags, not the custom `refs/f20-native-uat/*` evidence namespace. The proven-working prior runs (07-22 `5e665ec5`, 07-23 `6937ef61`) dispatched on **branches** `f20-native-uat-<full-sha>`. I therefore created the dispatch branch `f20-native-uat-95c81ec6…` at the same sealed SHA (making `github.sha == sealed SHA`, the brief's stated intent) and dispatched there. The 422 attempt created **no** run, so the single-dispatch invariant was preserved (exactly one run created). Both refs point at the same immutable sealed commit.

## Per-target native results

### Windows — job `89364776258`, runner `SEANDESKTOP` (runner_id 21, labels `[self-hosted, Windows, X64, msvc]`) — RED

- **Landed on the correct runner** (self-hosted msvc, NOT hosted `windows-2022`; the `windows-2022` soak job was correctly `skipped` in candidate mode).
- **Failed in the workflow setup step, before `f20-native-windows-proof.ps1` ran.** `git rev-parse "$env:EXPECTED_COMMIT^{tree}"` returned null because git refused to operate on the checkout:
  ```
  fatal: detected dubious ownership in repository at 'C:/actions-runner-core/_work/wayland-core/wayland-core'
  '.git' is owned by: BUILTIN\Administrators (S-1-5-32-544)
  but the current user is: NT AUTHORITY\NETWORK SERVICE (S-1-5-20)
  → git config --global --add safe.directory C:/actions-runner-core/_work/wayland-core/wayland-core
  InvalidOperation: … You cannot call a method on a null-valued expression.
  ```
- **Cause: self-hosted runner environment misconfiguration** (`safe.directory` not set for the `SEANDESKTOP` runner's work dir, which is owned by `BUILTIN\Administrators` while the runner service is `NT AUTHORITY\NETWORK SERVICE`). This is **not** a defect in the sealed candidate and **not** a `wcore-sandbox` Windows compile result. The Windows AppContainer/Job-Object properties and the `windows-f20-lifecycle` (`wcore-agent` on `x86_64-pc-windows-msvc`) compile were **neither proven nor disproven** — the proof never started.
- Per-target markers (`windows-retained-handle`, `windows-appcontainer-acl`, `windows-job-object`, `windows-public-dispatch`, `windows-hard-process-containment`, `windows-f20-lifecycle`): **none emitted**. Final `F20_NATIVE_WINDOWS_ACCEPTANCE`: **absent**.

### macOS — job `89364776205`, runner `f20-macos-ephemeral-Seans-MacBook-Pro` (runner_id 25, ephemeral, exact image label) — RED

- **Landed on the correct ephemeral runner**, which **deregistered after its one job** (confirmed absent from the repo runner list post-run; only `ferrox-win-msvc` + `SEANDESKTOP` remain).
- **Genuine compile error in the sealed tree** on the first target `macos-retained-directory` (`cargo … -p wcore-sandbox --features live-docker`). The full dependency tree built (bollard, wcore-sandbox, …) and then the `wcore-sandbox` lib test failed to compile:
  ```
  error[E0599]: no method named `rename_into` found for struct
    `directory_authority::DirectoryAuthority` in the current scope
    --> crates/wcore-sandbox/src/directory_authority_tests.rs:149:11
  149 |     mover.rename_into(&authority, "landed", false).unwrap();
  error: could not compile `wcore-sandbox` (lib test) due to 1 previous error
  error: command `cargo test --no-run … --package wcore-sandbox --features live-docker` exited with code 101
  native macOS target macos-retained-directory failed
  ```
- **Cause: substantive cross-platform compile debt in the sealed candidate.** `directory_authority_tests.rs:149` calls `DirectoryAuthority::rename_into`, which does not resolve when the `wcore-sandbox` lib tests are compiled on macOS with `--features live-docker`. The Linux 11509/0 seal (20-23) did not catch this — exactly the kind of platform-specific compile failure the native gate exists to surface (analogous to the 07-22 `wcore-sandbox` Windows compile failure R14 wanted re-proven).
- Per-target markers (8 macOS targets from `macos-retained-directory` through `macos-f20-lifecycle`): **none emitted** (failed on target 1). Final `F20_NATIVE_MACOS_ACCEPTANCE`: **absent**.

## Runner identities (recorded)

| Platform | Job | Runner | runner_id | Labels | Correct? | Post-run |
|----------|-----|--------|-----------|--------|----------|----------|
| Windows | 89364776258 | SEANDESKTOP | 21 | self-hosted, Windows, X64, msvc | Yes (AppContainer-capable self-hosted msvc) | still online |
| macOS | 89364776205 | f20-macos-ephemeral-Seans-MacBook-Pro | 25 | self-hosted, f20-native-macos, f20-ephemeral, f20-no-ambient-secrets, f20-image-2e61537b… | Yes (pinned ephemeral) | deregistered (one-job) |

## Retained-ref recheck

`git ls-remote … refs/f20-native-uat/95c81ec6…` → `95c81ec6a351ec22125497333739fa7c93a0cd8b` after capture. **Retained; deletion not authorized.** The dispatch branch `f20-native-uat-95c81ec6…` (same sealed SHA) is left in place; not deleted (no cleanup authorization).

## Explicit non-claims

- **No Phase-20 requirement is completed here** (the aggregate is 20-28). REQ-native-r14/r4/r7/r9/r11/r3 remain unproven.
- Native PASS is **not** claimed for either platform. No cross-compilation or source-inspection substitute for the missing hardware proof was used.

## 20-26 admission decision

**20-26 (fresh review) is NOT admitted.** A repaired successor is required, because:

1. **macOS is a genuine candidate defect (blocking):** the sealed candidate `95c81ec6` does not compile `wcore-sandbox` lib tests on macOS (`DirectoryAuthority::rename_into` missing at `directory_authority_tests.rs:149`). This must be fixed in source, re-sealed, and re-authorized — the current sealed tree cannot pass native macOS acceptance.
2. **Windows is an infra block (must be fixed before a re-dispatch can prove Windows):** the `SEANDESKTOP` runner needs git `safe.directory` set for `C:/actions-runner-core/_work/wayland-core/wayland-core` (or the workflow must set it) so `f20-native-windows-proof.ps1` can run. Windows was neither proven nor disproven.

**Required next step:** a repaired successor plan that (a) fixes the macOS `wcore-sandbox` compile error, (b) resolves the `SEANDESKTOP` `safe.directory` misconfiguration, (c) re-seals the candidate, and (d) obtains fresh Sean native-proof-dispatch authorization for the new sealed digest.

## Self-Check

- Sealed commit/tree verified before mutation: PASS (`95c81ec6^{tree} == 784f4980…`).
- Single dispatch invariant: PASS (exactly one `workflow_dispatch` run `30054999992` on the sealed SHA; the 422 attempt created no run; no retry).
- Retained ref: PASS (`refs/f20-native-uat/95c81ec6…` → sealed SHA post-capture).
- Honesty gate: PASS (both platforms RED, zero markers, no green faked, no requirement claimed).
