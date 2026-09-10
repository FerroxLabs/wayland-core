---
issue: 1324
repo: FerroxLabs/wayland
kind: task
title: "Core stabilization: verified lifecycle, credential, budget and acceptance repair program"
status: open
last_verified_commit: 1780b3164c6973279fd49d4dda17e8605f51d047
criteria:
  - id: c1
    text: "Each applicable package has exact source/artifact/fixture identity, fail-before/pass-after controls, actual executed commands/counts, cleanup, current integration evidence and an independent review."
    state: not-met
    owner: core
    note: "Programme remains incomplete: FINAL-PLAN.md and evidence/execution-progress.json under /Users/seandonahoe/dev/waylandcore-stabilization-20260905 remain the existing execution record. Partial package evidence does not discharge this whole-program criterion; this metadata correction does not re-grade implementations."
  - id: c2
    text: "External/unmeasured criteria remain visibly incomplete with their owning carriers."
    state: not-met
    owner: core
    handoff: "FerroxLabs/wayland#1349"
    note: "External disposition and final reconciliation remain incomplete. Desktop residual is #1323; AppContainer #368 remains under Q-368-honesty and maintainer #410. No closure or publication authorized."
---

Recorded from live #1324 on 2026-09-06. Tracker is OPEN, area:core, state:in-progress, with no milestone. Acceptance above follows the coordination handle; existing individual carriers retain their criteria. This entry records pending work, not a new implementation plan or release waiver.

Classification: the tracker describes a programme coordination handle, not an individual defect. Package completion and external residuals remain separate, and unmet criteria above remain unmet.

Squash provenance reconciliation: PR #455 head `28634521b854d58672819843865f6ab7dd16d72c` and integrated
commit `1780b3164c6973279fd49d4dda17e8605f51d047` have identical tree
`90e028ae6de0537c60c1d86450ceb3083ae4610b`. Original anchor `3d2089b45`
is an ancestor of the PR head; squash integration removed that ancestry. This
repin preserves the recorded grading and does not establish new runtime or
release acceptance. Exact-head CI `34242741624` passed before that squash.
