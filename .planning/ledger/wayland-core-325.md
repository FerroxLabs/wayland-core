---
issue: 325
repo: FerroxLabs/wayland-core
title: "nightly-windows-soak closes its tracker issue from a job that cannot see half the run's failures"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The tracker close is gated on the result of every job in the run, not on one job's step-level success()"
    state: not-met
    owner: core
    note: "the close step sits inside the windows-soak job with if success(), which means every prior step in THIS job passed. It cannot read a sibling job's result, so adding needs: to the existing steps does not fix it - report and close must move into a terminal job with if always()"
  - id: c2
    text: "A run whose sibling job failed posts a red report instead of closing the tracker green"
    state: not-met
    owner: core
    note: "this already fired. Run 33053333326 on 2026-08-27 concluded failure with windows-live-acceptance red, and issue #319 was auto-closed at 08:56:03Z with a bot comment reading Windows soak GREEN. That run is the recorded red arm to replay"
  - id: c3
    text: "windows-live-acceptance and keyring-blob-size are inside the tracker's sight, not just windows-soak"
    state: not-met
    owner: core
    note: "the issue body names two test jobs; there are three jobs outside the tracker's sight. windows-live-acceptance holds contents: read only and owns neither the report nor the close step"
  - id: c4
    text: "The existing label and title-prefix narrowing survives, so the reporter still cannot touch a human-filed issue"
    state: not-met
    owner: core
    note: "the fix must not widen what the bot can close while it widens what the bot can see. This is a property to preserve, and it is untested today"
---

The nightly Windows soak workflow closes its own failure-tracker issue from a
step inside one job, gated on that step's own success. A step-level success()
means every prior step in the same job passed - it says nothing about the other
jobs in the run. So a green soak job closes the tracker no matter what the
live-acceptance job or the keyring job did.

This is not hypothetical any more. On 2026-08-27 a run whose conclusion was
failure, with the live-acceptance job red on the AppContainer ACL race, posted
the word GREEN and closed the tracker. That is the laundering, measured and
dated, on the shipped lineage.

It is CI config only, no crate changes, and it is the cheapest of its cluster.
Both #324 and #350 depend on it: until it lands neither gets a durable tracker
row, which is exactly how #324 survived three nightlies with no issue filed.

Criteria come from the cluster C verification note of 2026-08-29, which read
the workflow at the shipped commit.
