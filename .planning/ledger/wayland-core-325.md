---
issue: 325
repo: FerroxLabs/wayland-core
kind: defect
title: "nightly-windows-soak closes its tracker issue from a job that cannot see half the run's failures"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The tracker close is gated on the result of every job in the run, not on one job's step-level success()"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:712:needs: [windows-soak, keyring-blob-size, windows-live-acceptance]"
    owner: core
    note: "A new terminal job soak-tracker: needs [windows-soak, keyring-blob-size, windows-live-acceptance] with if: always(). The close step is gated on the decision from .github/scripts/soak-tracker-decision.sh, which closes only when EVERY roster entry is success. Fail-closed extras: an empty or incomplete roster, or an uninterpretable needs.<id>.result, exits 1 and closes nothing; the soak job no longer holds issues: write."
  - id: c2
    text: "A run whose sibling job failed posts a red report instead of closing the tracker green"
    state: met
    evidence: "file:.github/scripts/tests/soak-tracker-run.test.py"
    owner: core
    handoff: "FerroxLabs/wayland-core#386"
    note: "RE-GRADED 2026-08-29. The unit case at soak-tracker-truth.test.sh:65 grades the DECISION SCRIPT and is genuinely non-vacuous (mutating the failure) case arm of soak-tracker-decision.sh takes that suite to 17 passed / 4 failed), but the criterion says a RUN posts a red report, and four pieces of plumbing sat between the two ungraded: the multi-line JOB_RESULTS block whose left-hand names are typed by hand beside needs.<id>.result, a mistyped needs id expanding to the empty string, the $GITHUB_OUTPUT -> steps.decide.outputs.action hand-off (the unit suite never sets GITHUB_OUTPUT at all, so emit() could write nothing and every case would still pass), and the github-script bodies that do the posting. soak-tracker-run.test.py now executes the real YAML end to end - real env interpolation, real decision script writing a real GITHUB_OUTPUT, real step if: expressions, real script bodies under node against a stubbed Octokit - and asserts on the API CALLS: a red sibling comments on the tracker naming windows-live-acceptance, opens one when none exists, and never issues issues.update(state closed); an all-green run closes and opens nothing. 30 assertions, run from PART C of the existing suite, which lint.yml already invokes. RED ARMS: cross-wiring one JOB_RESULTS line -> 1 fail; dropping the GITHUB_OUTPUT write from emit() -> soak-tracker-run 17 passed / 13 failed, and soak-tracker-truth 21 / 1, against 30 / 0 and 22 / 0 on the untouched tree (re-measured 2026-08-30 at 856df7d0; an earlier draft of this note said '6 fails', which is not what the tree produces -- the verifier's figure was the correct one and the arm is STRONGER than was claimed); gating the close step on report -> 4 fails. HANDOFF: GitHub\'s own scheduler admitting the job through if always() when a sibling is red is simulated, not observed, and the workflow has still never run with this job (every scheduled run is on main, which does not contain 2282de36). That needs one real dispatch, it writes to a live tracker issue, and it is #386. THE DISPATCH THE HANDOFF ASKED FOR HAS NOW RUN, and it closes the simulated half without closing #386. FerroxLabs/wayland-core run 33265083678, workflow_dispatch on lane/f13-n-soak-misc with tracker_rehearsal=true: GHA itself computed `OAuth-sized secret through the real Credential Manager (windows-2025) :: failure` with both siblings `skipped`, always() admitted the tracker job with a sibling red, the decision script read the real multi-line JOB_RESULTS built from the needs.<id>.result expressions, wrote $GITHUB_OUTPUT, and steps.decide.outputs.action resolved to `report` with `reason=job-failed`. Both workflow files are byte-identical to the run's own headSha f58906752 (nightly-windows-soak.yml cbb97bad, soak-tracker-truth.test.sh 2668c0d8), so the run graded this code and not an ancestor. WHAT THAT RUN DOES NOT SHOW, stated plainly because an earlier draft of this note overstated it as 'MET by a real workflow_dispatch run': tracker_rehearsal=true makes BOTH issue-writing steps inert, so the run demonstrates the DECISION and the always()/roster/needs plumbing, and ASSERTS rather than performs the posting -- it fails the run on any decision other than report, and refuses to grade at all if keyring-blob-size did not actually report failure. The POSTING verb is carried by soak-tracker-run.test.py above, which drives the real script bodies under node against a stubbed Octokit and asserts on the API calls themselves. Neither instrument covers the criterion's sentence alone; together they do, and what remains unobserved is only a write to a live tracker issue, which is #386 and Sean's call. THE REHEARSAL PATH IS NOW GRADED OFF GITHUB TOO, which is what makes the paragraph above more than a report of one run. Merging this lane onto the tip put the rehearsal gating and the end-to-end YAML harness in the same tree for the first time, and the harness REFUSED the compound gate rather than skipping it: `step 'Close the failure issue on a green RUN' is gated on "${{ steps.decide.outputs.action == 'close' && github.event.inputs.tracker_rehearsal != 'true' }}", which this harness cannot evaluate. Teach it the new form rather than letting the step go silently ungraded.` That refusal is the right behaviour and it was taken as an instruction: the harness now parses an `&&` chain of decide-output and dispatch-input clauses and still raises on anything it cannot read, and the acting-step roster count is derived from the YAML instead of the literal 3 it was pinned to, so adding a step without a JOB_RESULTS block fails there rather than silently moving the expected number. Ten new cases drive the REHEARSAL: with keyring-blob-size red it still decides `report`, the verdict step runs, NEITHER issue-writing step runs, nothing at all is written to the tracker, and the verdict exits 0; with a different sibling red the verdict REFUSES to grade and says the premise is absent; all-green it refuses too and closes nothing. FOUR RED ARMS on the artefacts themselves, each anchor read back and confirmed to be executable YAML or shell and not a comment: make the premise guard a no-op (the two premise cases redden), make the decision script count a failure as zero so a red sibling decides `close` while the premise still holds (the verdict-passes case reddens, which is the `core#325 REGRESSED` guard firing), and drop `tracker_rehearsal` from the report step's gate and from the close step's gate (the two inertness cases and the all-green case redden respectively). Restored byte-identical, 43 passed / 0 failed, and soak-tracker-truth 26 passed / 0 failed. So the posting verb is now covered end to end in BOTH modes -- the live-mode API calls against a stubbed Octokit, and the rehearsal mode's inertness -- and the real dispatch supplies the one thing neither can: GitHub's own scheduler admitting the job through always() with a sibling red."
  - id: c3
    text: "windows-live-acceptance and keyring-blob-size are inside the tracker's sight, not just windows-soak"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:728:REQUIRED_JOBS: "windows-soak keyring-blob-size windows-live-acceptance""
    owner: core
    note: "REQUIRED_JOBS names all three; a job id missing from the roster exits 1 with 'Incomplete soak roster'. soak-tracker-truth.test.sh:162-201 derives the scheduled-job set from the YAML and reddens if REQUIRED_JOBS or needs: drifts from it, so a fourth job cannot narrow the view silently."
  - id: c4
    text: "The existing label and title-prefix narrowing survives, so the reporter still cannot touch a human-filed issue"
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml:763:no open soak failure issue - nothing to close"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: same line, but that `issues.find` predicate is written twice (:723 close path, :810 open path) and matched both; the fragment is now unique to the CLOSE path this criterion is about. labels: ['windows-soak','test-debt'] plus a title startsWith('[nightly-windows-soak] FAIL') narrowing, repeated on the report step. PRESERVED BUT STILL UNTESTED: nothing under .github/scripts/ grades the label or title narrowing, so a later edit could widen what the bot may close without reddening anything."
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
