---
issue: 443
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-09-04"
status: closed
last_verified_commit: 1780b3164c6973279fd49d4dda17e8605f51d047
criteria:
  - id: c1
    text: "The nightly Windows soak failure is triaged: either the failing job is fixed, or the run is shown to have failed for an infrastructure reason and the issue is closed."
    state: met
    evidence: "commit:1780b3164c6973279fd49d4dda17e8605f51d047"
    owner: core
    note: "Original run 33841172783 at509f4426b failed live_fs_acl::concurrent_allow_and_deny_identities_do_not_interfere: ordinary allow identity must retain access;12/13 passed. This is the concurrent ACL grant defect repaired by accepted W14 commit3abeeef2b. Its native Windows receipt executes this exact case successfully and all13 ACL cases pass with retries0. The issue was independently auto-closed on2026-09-07 after whole-nightly run34087589551 at0cdd998ad passed. That later nightly is closure metadata, not the proof of our repair and not evidence for the current release candidate. The original failure and both source identities remain preserved. This triage does not close or regrade the wider#368/#410 criteria."
---

# Historical nightly failure triaged against the accepted ACL repair

Original failure: https://github.com/FerroxLabs/wayland-core/actions/runs/33841172783

Automation closure: https://github.com/FerroxLabs/wayland-core/issues/443#issuecomment-5572032943

The original live-acceptance log identifies the same concurrent allow/deny
identity assertion exercised by W14. The accepted repair changes protected-deny
handling and lease cleanup to preserve other identities' grants. Native proof
at `3abeeef2bf27a35457b0aa2e2df3204f551037d4` records the named case passing in
7.540s and all13 ACL cases passing without retries. The stabilization execution
record retains the original log and the W14 JUnit/source receipts separately.

The final-tag native collector remains required; this historical triage does
not replace current release qualification.

Squash provenance reconciliation: PR #455 head `28634521b854d58672819843865f6ab7dd16d72c` and integrated
commit `1780b3164c6973279fd49d4dda17e8605f51d047` have identical tree
`90e028ae6de0537c60c1d86450ceb3083ae4610b`. Original anchor `3abeeef2bf27a35457b0aa2e2df3204f551037d4`
is an ancestor of the PR head; squash integration removed that ancestry. This
repin preserves the recorded grading and does not establish new runtime or
release acceptance. Exact-head CI `34242741624` passed before that squash.

Historical status before the 2026-09-09 closure: GitHub #443 was OPEN in milestone 0.13.14. Run `34191383717` at
`0cdd998adb1cd2ab13966a40b1b98c6136fd72d0`, job `101949990471`, records
`concurrent_allow_and_deny_identities_do_not_interfere` failing attempt 1 with
"ordinary allow identity must retain access", then passing attempt 2; summary:
13 passed, 1 flaky. This older-source recurrence does not invalidate preserved
W14 repair evidence, but does not establish the issue closure criterion. The
evidence commit identifies integrated source only; final-tag native
qualification remains required.

Current tracking reconciliation: #443 was automatically closed at 2026-09-09T06:24:59Z after run34315645505 on integrated source1780b3164. The three required nightly jobs passed. This closure plus the preserved W14 repair supports the triage criterion; it does not replace final-source native release acceptance. No issue state or milestone changed by this reconciliation.
