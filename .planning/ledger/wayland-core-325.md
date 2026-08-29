---
issue: 325
repo: FerroxLabs/wayland-core
kind: defect
title: "nightly-windows-soak closes its tracker issue from a job that cannot see half the run's failures"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The tracker close is gated on the result of every job in the run, not on one job's step-level success()"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:676"
    owner: core
    note: "A new terminal job soak-tracker: needs [windows-soak, keyring-blob-size, windows-live-acceptance] with if: always(). The close step is gated on the decision from .github/scripts/soak-tracker-decision.sh, which closes only when EVERY roster entry is success. Fail-closed extras: an empty or incomplete roster, or an uninterpretable needs.<id>.result, exits 1 and closes nothing; the soak job no longer holds issues: write."
  - id: c2
    text: "A run whose sibling job failed posts a red report instead of closing the tracker green"
    state: met
    evidence: "file:.github/scripts/tests/soak-tracker-truth.test.sh:65"
    owner: core
    note: "Replays the exact shape of run 33053333326 as a unit case - soak green, keyring green, live-acceptance red -> report, never close - with a negative control at :59 (all three green -> close) so the guard cannot be one that never PASSes. Shell script, so file: rather than symbol:. This is the red arm replayed, not a re-run of the historical workflow."
  - id: c3
    text: "windows-live-acceptance and keyring-blob-size are inside the tracker's sight, not just windows-soak"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:692"
    owner: core
    note: "REQUIRED_JOBS names all three; a job id missing from the roster exits 1 with 'Incomplete soak roster'. soak-tracker-truth.test.sh:162-201 derives the scheduled-job set from the YAML and reddens if REQUIRED_JOBS or needs: drifts from it, so a fourth job cannot narrow the view silently."
  - id: c4
    text: "The existing label and title-prefix narrowing survives, so the reporter still cannot touch a human-filed issue"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:723"
    owner: core
    note: "labels: ['windows-soak','test-debt'] plus a title startsWith('[nightly-windows-soak] FAIL') narrowing, repeated on the report step. PRESERVED BUT STILL UNTESTED: nothing under .github/scripts/ grades the label or title narrowing, so a later edit could widen what the bot may close without reddening anything."
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
