---
issue: 1116
repo: FerroxLabs/wayland
kind: defect
title: "Desktop: no abandon verb on the host wire leaves a crash-interrupted session wedged in-app"
status: open
last_verified_commit: 1780b3164c6973279fd49d4dda17e8605f51d047
criteria:
  - id: c1
    text: "A crash-interrupted session can be ended through the host wire and Desktop recovery surface while preserving unknown external-effect outcomes."
    state: not-met
    owner: core
    note: "The original issue has no declared criterion IDs; this row records its unresolved recovery requirement without claiming completion. ResumeTurnAction already includes Abandon in the tagged source, so the historical title is not a current missing-enum diagnosis. The latest Core handoff reports that an unknown started effect still prevents the requested recovery disposition. Live issue remains open in milestone 0.13.14; no failed outcome is relabelled succeeded, failed or not-started."
  - id: c2
    text: "The Desktop consumer handles the host-stop recovery behavior and verifies the resulting recovery control."
    state: not-met
    owner: core
    note: "Core retains coordination ownership until the shared recovery contract is resolved; Desktop implementation and consumer acceptance are not established by this release. The issue remains the existing cross-lane carrier."
---

Release metadata reconciliation only. Sources: https://github.com/FerroxLabs/wayland/issues/1116 and its 2026-09-08 coordination comments. Both requirements remain unmet. The live milestone is 0.13.14, verified before this record was written; no milestone, issue state, runtime behavior or release threshold is changed here.

Tagged source: 28634521b854d58672819843865f6ab7dd16d72c. The release metadata revision does not alter that tag, its generated Desktop contract, or its qualified binaries.

Provenance: the existing publisher metadata entry is retained with both criteria
not met. Its source tag `28634521b854d58672819843865f6ab7dd16d72c` and
squash integration `1780b3164c6973279fd49d4dda17e8605f51d047` share tree
`90e028ae6de0537c60c1d86450ceb3083ae4610b`; this pointer reconciliation adds
no runtime proof and does not waive the recorded limitation.
