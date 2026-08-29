---
issue: 325
repo: FerroxLabs/wayland-core
kind: defect
title: "nightly-windows-soak closes its tracker issue from a job that cannot see half the run's failures"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The tracker close is gated on the result of every job in the run, not on one job's step-level success()"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:676"
    owner: core
    note: "A new terminal job soak-tracker: needs [windows-soak, keyring-blob-size, windows-live-acceptance] with if: always(). The close step is gated on the decision from .github/scripts/soak-tracker-decision.sh, which closes only when EVERY roster entry is success. Fail-closed extras: an empty or incomplete roster, or an uninterpretable needs.<id>.result, exits 1 and closes nothing; the soak job no longer holds issues: write."
  - id: c2
    text: "A run whose sibling job failed posts a red report instead of closing the tracker green"
    state: not-met
    evidence: "file:.github/scripts/tests/soak-tracker-truth.test.sh:65"
    owner: core
    note: "Replays the exact shape of run 33053333326 as a unit case - soak green, keyring green, live-acceptance red -> report, never close - with a negative control at :59 (all three green -> close) so the guard cannot be one that never PASSes. Shell script, so file: rather than symbol:. This is the red arm replayed, not a re-run of the historical workflow. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: The DECISION LOGIC holds and is non-vacuous, but the criterion as written says 'A RUN whose sibling job failed posts a red report', and the ticket's own close condition is explicit: 'Any fix needs a red arm: a RUN where one job is green and the other red must be shown to not close the tracker issue.' No run has ever executed this code. `gh run list --workflow nightly-windows-soak.yml` shows every scheduled run is `headBranch: main`; `git branch -r --contains 2282de36` lists integ/f13, integ/next and lane/* but NOT origin/main, and origin/main's last touch of the workflow is d3a4ea00 (pre-fix). The most recent run 33236533548 (2026-08-29) has 5 jobs and no `Soak tracker (whole-run truth)` job — the laundering wiring is still what actually runs nightly. What DOES resolve: `file:.github/scripts/tests/soak-tracker-truth.test.sh:65` is exactly `decide 'soak green + live-acceptance red -> report, never close' report 0` and the negative control at :59 is `decide 'all three jobs green -> close' close 0`, so the guard is not one that never PASSes. I ran the suite on hetzner: 21 passed, 0 failed. MUTATION PROOF the case is real: in a scratch copy I changed lines 101-102 of soak-tracker-decision.sh (inside the `failure)` case arm — verified it landed on CODE, not a comment, by printing lines 96-105 before and after) from `FAILED=$((FAILED + 1))` / `NONSUCCESS=$((NONSUCCESS + 1))` to `+ 0`; the suite went 17 passed / 4 failed with `FAIL soak green + live-acceptance red -> report, never close (want action=report; got action=close reason=all-green)`. So the replay is a genuine red arm for the SCRIPT. What is untested by execution is the GHA plumbing the ticket asked to see: the multi-line `JOB_RESULTS: |` interpolation of `${{ needs.<job>.result }}`, `$GITHUB_OUTPUT` → `steps.decide.outputs.action`, and `always()` admitting the tracker when a sibling is red. Given #325 exists precisely because a condition's SCOPE was misread, the new scoping deserves one real run. Remainder is cheap: `workflow_dispatch` is declared (:20) and prior dispatches from non-default refs (fix/ci-reds-rc3, v0.12.26) ran fine, so one dispatch on the fix ref would exercise it — note it would open/comment on a real tracker issue, so that is Sean's call."
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
