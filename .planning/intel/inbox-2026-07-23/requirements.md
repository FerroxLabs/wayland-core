# Requirements (native-repair R1–R12)

Extracted from the SPEC `native-uat-repair-BRIEF.md` §6 (precedence 0,
manifest-authoritative). These are NEW, non-overlapping additions scoped to the
Phase-20 native-UAT repair; they do not duplicate the existing F20-01…F20-06 /
F20-GATE-01/02 requirements in `.planning/REQUIREMENTS.md`. They refine Phase-20
Success Criterion #3 (the "native Windows/macOS identities share one
authoritative lifecycle" clause) with concrete, evidence-backed acceptance.

## REQ-native-r1-is-available
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.A / §4.A
- description: `AppContainerBackend::is_available()` returns true on a real AppContainer-capable Windows host (add `.write(true)` to `storage.rs` `create_new_nofollow` OpenOptions chain).
- acceptance: Probe returns exit 0 and `is_available()` → true on real Windows 11 hardware.
- scope: wcore-sandbox appcontainer/acl_lease/storage.rs

## REQ-native-r2-sandbox-read-grant-revoke
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.A / §4.B
- description: A sandboxed process can read `fs_read_allow`-granted files and is denied ungranted files, with grants revoked after the run, isolation preserved (drop deny-only SidsToDisable in CreateRestrictedToken).
- acceptance: `granted_path_is_readable_then_revoked` green on real hardware; a genuine DENY ace still blocks; a file granted only to normal SIDs is still denied.
- scope: wcore-sandbox appcontainer/windows_impl/process.rs

## REQ-native-r3-agent-compiles-windows
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.A / §4.C
- description: `wcore-agent` compiles on `x86_64-pc-windows-msvc` (import `READ_CONTROL`/`WRITE_DAC` from `Win32::Storage::FileSystem`, not `Win32::Security`).
- acceptance: `windows-f20-lifecycle` target builds; E0432 resolved.
- scope: wcore-agent session_journal/snapshot.rs:654

## REQ-native-r4-no-regression
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.A / §4.4
- description: No regression to Linux or macOS from the native fixes.
- acceptance: Linux `nextest --profile ci` stays 11509/0; macOS native proof stays 8/8.
- scope: cross-platform aggregate

## REQ-native-r5-type-and-hold
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.B / §4.D
- description: `type_and_hold` asserts on the granted read's success (exit 0), not `choice.exe`'s 1-based selection index; hold must stay stdin-free.
- acceptance: `live_fs_acl.rs::one_execution_grant_never_leaks_to_another_identity` passes with a stdin-free exit-0 delay primitive.
- scope: wcore-sandbox tests/live_fs_acl.rs

## REQ-native-r6-dispatch-smoke-portable
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.B / §4.E
- description: `dispatch_smoke` is Windows-portable (drop open handles before rename / restructure) or the Windows target is narrowed to the acceptance-relevant test.
- acceptance: `dispatch_rejects_different_head_repository_replacement` no longer panics with Os code 5 PermissionDenied on Windows.
- scope: wcore-swarm tests/dispatch_smoke.rs:289

## REQ-native-r7-windows-jobobject-tests
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.C / §4.F (needs research §7.1)
- description: `windows-job-object` and `windows-hard-process-containment` proof targets map to REAL Windows Job-Object containment tests (must be authored; none exist today).
- acceptance: Both targets exercise the real Windows mechanism (KILL_ON_JOB_CLOSE, active-process cap, breakaway denial, exit-code fidelity, descendant reaping with no residue) and pass on a self-hosted AppContainer runner.
- scope: scripts/f20-native-windows-proof.ps1 + new wcore-sandbox Windows containment tests

## REQ-native-r8-antidrift-guard
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.C / §4.G (needs research §7.4)
- description: A structural guard so a Windows proof target cannot map to a non-Windows test (and vice-versa for macOS) — e.g. per-target `#[cfg(target_os=...)]` assertions or a harness lint.
- acceptance: A native target mapped to the wrong-OS test fails fast / is rejected by the guard.
- scope: native proof harness

## REQ-native-r9-macos-revalidate
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.C / §4.G (needs research §7.3)
- description: Re-validate the macOS proof harness against real macOS (re-confirm the 8 targets are real + green; fix any aspirational mappings).
- acceptance: All 8 macOS native targets confirmed real and green on the ephemeral Scaleway Apple-silicon runner.
- scope: scripts/f20-native-macos-proof.sh

## REQ-native-r10-cargo-lock
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.D / §5
- description: Regenerate and commit `Cargo.lock` (stale at `be84bd2`; build adds cap-std/serde_json/tar to wcore-sandbox and fd-lock/libc/sha2/windows-sys to wcore-swarm).
- acceptance: Cargo.lock consistent and committed on the candidate branch.
- scope: repo hygiene

## REQ-native-r11-selfhosted-runner
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.D / §8 infra
- description: The Windows proof leg runs on an AppContainer-capable self-hosted runner (client SKU, labels `self-hosted, Windows, X64, msvc`), not hosted `windows-2022`; workflow `runs-on` changes accordingly.
- acceptance: Windows native leg dispatches to the self-hosted msvc runner.
- scope: CI workflow (nightly-windows-soak.yml runs-on)

## REQ-native-r12-full-gate-sequence
- source: .planning/inbox/native-uat-repair-BRIEF.md §6.D / §9
- description: The repaired candidate goes through the full gate sequence: build → cross-audit → Hetzner aggregate → native proof (Windows + macOS) → FRESH 20-16 independent review (native NOT deferred) → 20-17 re-prep → Sean-authorized 20-18 native UAT.
- acceptance: All gates green on one exact candidate SHA; fresh 20-16 with no deferred native; 20-17 re-prep; Sean-authorized 20-18 dual-platform green.
- scope: Phase 20 native path closure
