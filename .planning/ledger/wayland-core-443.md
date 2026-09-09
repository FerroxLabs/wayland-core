---
issue: 443
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-09-04"
status: open
last_verified_commit: 3abeeef2bf27a35457b0aa2e2df3204f551037d4
criteria:
  - id: c1
    text: "The nightly Windows soak failure is triaged: either the failing job is fixed, or the run is shown to have failed for an infrastructure reason and the issue is closed."
    state: not-met
    evidence: "commit:3abeeef2bf27a35457b0aa2e2df3204f551037d4"
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

Release metadata reconciliation 2026-09-09: GitHub issue #443 was reopened on 2026-09-08 and remains open in milestone 0.13.14. The reopening cites run 34191383717 at old source 0cdd998adb1cd2ab13966a40b1b98c6136fd72d0, not the qualified v0.13.13 source. Preserve the historical accepted W14 repair evidence above, but do not claim the issue closure criterion is satisfied. Final tagged native qualification remains required; neither the old nightly nor this metadata revision substitutes for it.
