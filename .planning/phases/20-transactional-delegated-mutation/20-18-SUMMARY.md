---
phase: 20-transactional-delegated-mutation
plan: "18"
type: execute
status: incomplete
completed: 2026-07-22
disposition: INCOMPLETE
requirements_completed: []
requirements_incomplete: [F20-01, F20-02, F20-03, F20-04, F20-05, F20-06, F20-GATE-01, F20-GATE-02]
# Exact reviewed candidate (20-16) and prepared tuple (20-17)
source_sha: 5e665ec5911fa2a118de70b498b8f0e2841d50ba
source_tree: e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba
review_sha: 33c7938d2f47c165e73410c99733445d5772260c
pending_digest: 6ed9642cc867a3dff71f7fcb37073303636f8a675797a3a4f68c3b5f24e3adb3
nonce: c3b81287aec540c86ca4a632d371324c
# Authorization + single dispatch (states kept distinct)
authorized_by: sean
authorization: "authorize 6ed9642cc867a3dff71f7fcb37073303636f8a675797a3a4f68c3b5f24e3adb3 (digest recomputed from unchanged pending bytes; matched)"
uat_ref: refs/f20-native-uat/5e665ec5911fa2a118de70b498b8f0e2841d50ba
uat_ref_state: retained
uat_ref_deletion: deletion_not_authorized
dispatch_run: 29886894436
dispatch_ref: refs/heads/f20-native-uat-5e665ec5911fa2a118de70b498b8f0e2841d50ba
native_windows: FAILED
native_macos: FAILED
aggregate_test: not_run
aggregate_build: not_run
---

# Phase 20 Plan 18: Terminal Native Acceptance — INCOMPLETE (native proof red)

**Sean's exact tuple-specific authorization drove one publication and one `nightly-windows-soak.yml` dispatch against the exact independently-reviewed candidate `5e665ec`. Both native jobs completed with `failure`, on real platform defects. Native proof is red, so aggregate proof did NOT run and NO Phase 20 requirement completes. A repaired successor must return to 20-08 → fresh 20-16 review → 20-17 preparation → new authorization.**

## Distinct states (all separately identified)

- **Implementation / review / preparation / authorization:** all green. Source `5e665ec` passed the zero-finding `f20-16` review (`33c7938`); 20-17 persisted the exact pending tuple; Sean authorized the exact digest `6ed9642c…`.
- **Native UAT:** RED. One dispatch (run `29886894436`, `head_sha=5e665ec`, `event=workflow_dispatch`, not pre-existing), bound and validated. Both candidate jobs ran and failed; the scheduled soak was correctly skipped. The ephemeral macOS runner ran its one job and self-deregistered.
- **Aggregate Hetzner test/build:** NOT RUN (gated behind green native proof).
- **Requirement completion:** NONE. All of F20-01..06 / GATE-01 / GATE-02 remain incomplete.

## Findings (real candidate defects the native UAT exposed)

1. **Windows — `wcore-sandbox` lib does not compile** (hosted `windows-2022`, rustc 1.95.0-msvc). Target `windows-retained-handle` (`--test live_fs_acl`) failed at lib build with `E0425` (`reserve_output`, `probe_single_flight`, `BUFFERED_OUTPUT_LIMIT_BYTES` not found in `super`/`super::super`) and `E0423` (tuple struct with private fields). Windows-only cfg-gating regression — consistent with the parked `windows_impl` module-path debt that was never in the reviewed candidate. Blocks every Windows target.
2. **macOS — proof references a Windows-only test** (Scaleway M1, macOS 15.6.1, live colima docker). Target `macos-retained-directory` maps to `-p wcore-sandbox --test live_integrity`, but `live_integrity.rs` is `#![cfg(windows)]` → the binary has 0 tests → `--no-tests=fail` fails. `wcore-sandbox/tests/` contains only `live_fs_acl.rs` + `live_integrity.rs` (both Windows-oriented); there is no macOS retained-directory acceptance test for the proof to run. The `scripts/f20-native-macos-proof.sh` target→test mapping is broken (and possibly the macOS acceptance tests are unwritten).

Neither defect is reachable from Linux Hetzner proof or exact-source review — only native execution surfaces them. The gate worked as intended.

## Infra note (not a finding)

Toolchain and runner were sound: the candidate compiled on both platforms up to the defects, docker was live on the ephemeral mac, and the ephemeral runner selected the job and deregistered cleanly. The failures are candidate defects, not environment.

## Evidence retention

The UAT ref `refs/f20-native-uat/5e665ec…` is **retained** on `FerroxLabs/wayland-core` as immutable evidence of this attempt; its remote SHA still equals the candidate. **Deletion is not authorized** (requires a separate exact-ref Sean authorization + an unchanged-remote-SHA recheck). The temporary dispatch branch `f20-native-uat-5e665ec…` is deleted (its objects persist via the retained UAT ref). Git-private authority objects (request/authorization/publication/dispatch-intent/run-binding) remain under `.git/f20-native-uat/<sha>/`.

## Repair path (returns to 20-08)

A repaired successor must: (a) fix the Windows `wcore-sandbox` compile (define/gate `reserve_output`/`probe_single_flight`/`BUFFERED_OUTPUT_LIMIT_BYTES` + the private-field init for Windows; apply the parked `windows_impl` fix); (b) fix `f20-native-macos-proof.sh` target→test mapping and/or author the missing macOS acceptance tests; (c) re-prove focused + full suites on Hetzner; then re-enter fresh **20-16** review → **20-17** preparation → new tuple-specific **Sean authorization** → **20-18** re-run. The prior review/prep/authorization do not carry over to a new source SHA.

---
*Phase: 20-transactional-delegated-mutation*
*Disposition: INCOMPLETE — native proof red, no requirement completed*
*2026-07-22*
