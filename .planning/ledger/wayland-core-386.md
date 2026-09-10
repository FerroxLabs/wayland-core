---
issue: 386
repo: FerroxLabs/wayland-core
kind: defect
title: "core#325 c2 remainder: one real nightly-windows-soak run with a red sibling"
status: open
last_verified_commit: 5ca1e7857
criteria:
  - id: c1
    text: "One real nightly-windows-soak.yml run with a genuinely red sibling job is shown to NOT close the tracker issue -- observed on GitHub, against the live Octokit, not against the stub."
    state: met
    evidence: "file:.planning/RECORD-w15-gateB-2026-09-10.md:48:a run with a red sibling closed nothing, observed on GitHub rather"
    owner: core
    note: "MET 2026-09-10, and the finding is that the run this issue asks for ALREADY HAPPENED and nobody had gone looking. The body says the fix `has never executed on GitHub, and will not until it reaches main`; it reached main, and the 2026-09-04 scheduled tick is the run. RUN 33841172783 -- event schedule, branch main, head 509f4426b968a363248beb70d28223ff418e5c62 -- roster as the tracker itself read it: windows-soak=success, keyring-blob-size=success, windows-live-acceptance=failure. That is one real run with a genuinely red sibling and a green one. Job 100929260910 `Soak tracker (whole-run truth)` was PRESENT and did not skip; step 3 log verbatim: `jobs read : 3` / `failed : 1` / `uninterpretable: 0` / `action=report` / `reason=job-failed`. STEP 4 `Close the failure issue on a green RUN` -> SKIPPED, read from the job API rather than inferred from a log that omits skipped steps. Nothing was closed, against the live Octokit, not against the stub. POSITIVE CONTROL so the skip is a verdict and not a dead step: run 34087589551 (2026-09-07), same workflow, same live Octokit, roster all success -> action=close, reason=all-green, step 4 RAN, log `closed #443 on a green run`. NEGATIVE CONTROL: runs 33947609921 and 34014381034, windows-live-acceptance=cancelled -> action=none, reason=not-conclusive, steps 4, 5 AND 6 all skipped. Recorded on the issue at https://github.com/FerroxLabs/wayland-core/issues/386#issuecomment-5614409550 , which also supplies the fourth acceptance bullet (the run id and the issue number). LIMIT: no `[nightly-windows-soak] FAIL` issue was open at 06:08Z on 2026-09-04, so the close was refused at the DECISION step, before the issue lookup ran; the positive control above is what shows the lookup-and-close half is live. PREVIOUS: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed."
  - id: c2
    text: "That same run posts a comment on the tracker naming windows-live-acceptance, and opens one if none is open."
    state: met
    evidence: "file:.planning/RECORD-w15-gateB-2026-09-10.md:57:It opened FerroxLabs/wayland-core#443 at 2026-09-04T06:08:04Z"
    owner: core
    note: "MET 2026-09-10 by the same run 33841172783. Step 6 `Report red result to issue tracker` -> SUCCESS, and its log carries the live call `POST https://api.github.com/repos/FerroxLabs/wayland-core/issues`. It OPENED FerroxLabs/wayland-core#443 at 2026-09-04T06:08:04Z -- the same second as the step -- titled `[nightly-windows-soak] FAIL - 2026-09-04`, labels windows-soak + test-debt, body reading `**Failing job(s)**: windows-live-acceptance` and listing the roster line `windows-live-acceptance=failure`. So the tracker was told, and it was told WHICH job. The issue stayed OPEN for three days and was then auto-closed by the next all-green tick (run 34087589551), which is the behaviour the issue body's own acceptance describes. THE LIMIT, and it is a real one: this run exercised `issues.create`, because no tracker issue was open. The `issues.createComment` branch -- a red run adding to an ALREADY-OPEN tracker issue -- has still only ever run against the stubbed Octokit in soak-tracker-run.test.py. The criterion and the issue body both phrase this as a disjunction (`posts a comment ... and opens one if none is open`; `opened or commented on`), so what is bounded here is the BREADTH of the live evidence, not its existence. Recorded on the issue at https://github.com/FerroxLabs/wayland-core/issues/386#issuecomment-5614409550 . PREVIOUS: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed."
  - id: c3
    text: "If a real red sibling cannot be produced on demand, that is RECORDED as the ceiling and the stubbed-Octokit evidence is stated as what it is. The 30 assertions and three red arms already committed are strong, and they are not narrated as equivalent to a live run."
    state: met
    evidence: "file:.planning/RECORD-w15-gateB-2026-09-10.md:78:CEILING: a red sibling cannot be produced ON DEMAND by any lane"
    owner: core
    note: "MET 2026-09-10 as a RECORD, which is exactly what this criterion asks for, and posted on the issue first per this ledger's own rule: https://github.com/FerroxLabs/wayland-core/issues/386#issuecomment-5614409550 . THE CEILING: a red sibling cannot be produced ON DEMAND by any lane, and the run that discharges c1/c2 was produced by nature rather than to order. Two reasons, neither fixable from a branch. (a) The red came from `windows-live-acceptance`, which runs on the self-hosted ferrox-win-msvc Windows runner; nothing in a lane's control makes that job fail on request. (b) The on-demand route that DOES exist -- the `tracker_rehearsal` workflow_dispatch input -- deliberately replaces BOTH issue-writing steps with a step that PRINTS the payload and asserts action == report. It drives the real needs.<job>.result expansion and the real decision script and never touches the live Octokit. It is a rehearsal and the workflow says so. THE STUBBED-OCTOKIT EVIDENCE, STATED AS WHAT IT IS rather than narrated as equivalent: .github/scripts/tests/soak-tracker-run.test.py -- 30 assertions, three red arms, PART C of soak-tracker-truth.test.sh, run by lint.yml -- drives the real YAML, the real JOB_RESULTS interpolation, the real soak-tracker-decision.sh and the real actions/github-script bodies under node against a STUBBED Octokit. It grades the decision arithmetic and the script bodies. It cannot grade GitHub's scheduler admitting the tracker job through if: always() while a sibling is red, which is the one thing this issue was opened for and the one thing run 33841172783 supplies. LIMIT: this is a record, not a measurement; it asserts what cannot be produced on demand and why, and the reader has to take the two named mechanisms on the workflow's own text. PREVIOUS: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

Everything executable off GitHub was already done when this was filed: the
test drives the real workflow, the real interpolation, the real decision
script and the real github-script bodies under node against a stubbed
Octokit. What remained was the one thing a stub cannot supply.

ALL THREE CRITERIA ARE MET AS OF 2026-09-10, and the run that supplies them
was not caused by any lane — it already existed. The 2026-09-04 scheduled
tick (run `33841172783`) had `windows-live-acceptance` red beside two green
siblings, decided `action=report`, SKIPPED the close step and opened
FerroxLabs/wayland-core#443 against the live Octokit. The ticket had been
reading as owing a Sean action for eleven days that it no longer owed, which
is its own lesson: a criterion whose discharge depends on a scheduled event
needs somebody to go and look. Closing the issue is a release/Sean action,
not this lane's.

This sits close to kind: task -- it needs a real red job on a real platform.
It is written defect because c3 is code and judgement, not a credential a
human obtains, and because the gate's own rule is that an ambiguous entry is
written defect: over-blocking costs a conversation, under-blocking ships.
