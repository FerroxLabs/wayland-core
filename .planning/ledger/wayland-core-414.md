---
issue: 414
repo: FerroxLabs/wayland-core
kind: defect
title: "gate-admission.py fails 5 of its own assertions on the shipping branch, and has for some time"
status: closed
last_verified_commit: 48e74f447
criteria:
  - id: c1
    text: "Each of the five failing assertions is either FIXED, or declared with the reason it is permanently inapplicable -- decided one at a time, not waved through as a block."
    state: met
    evidence: "commit:93ede3424"
    owner: core
    note: "MET at 93ede3424 (the 0.13.12 merge, PR #417) and RE-VERIFIED on the current shipping tree. THE VERDICT FOR ALL FIVE IS THE SAME AND IT IS THE UNCOMFORTABLE ONE: the ASSERTIONS WERE RIGHT AND ci.yml WAS WRONG. Not one of the five was a stale expectation, so not one was weakened. PROVEN BY A BYTE-IDENTICAL A/B rather than by reading the diff: gate-admission.py from the current tree (sha256 28ed899af4a30c33..., the SAME file in both arms) was run against ci.yml reverted to 852f5acaa and produced `passed: 44 failed: 5` -- the identical five, same offenders -- then against the current ci.yml and produced 0. Gate held constant, production varied, verdict flipped. The only change to gate-admission.py between those two commits is PURELY ADDITIVE (a sixth rule, checkout-ordering); nothing was deleted, loosened or renamed, which is what makes the A/B decisive rather than suggestive. DECIDED ONE AT A TIME. (1) `every caller of an unconditional gate is admitted by always() or !cancelled()` -- the report job's `Assert test evidence exists` step was admitted by a hand-written `(needs.ci.result != 'cancelled' && != 'skipped') || (needs.ci-linux...)`. That expression is FALSE when both legs were skipped or cancelled, so the step SKIPS, and a skipped step is not a failure: `report` could conclude success having graded nothing -- the wayland#1115 shape the file exists to forbid. FIXED at source: the step now carries `if: ${{ !cancelled() }}`, and the offending form is now ABSENT from ci.yml (grep count 0). (2) `in an unconditional job, every step up to its last gate is unconditional too` -- four offenders in the same job (the aggregate step, `actions/checkout`, `Download nextest JUnit artifacts`, and the evidence gate itself) carried NO `if:` at all, which GitHub Actions reads as implicit `success()`. FIXED: all four now carry `!cancelled()`. (3) `every ci.yml job is aggregated by report or declared not-aggregated` -- `build-darwin-selfhosted` was unaccounted. Resolved by the DECLARATION branch this criterion explicitly allows, not by a wave-through: ci.yml now carries `# not-aggregated: build-darwin-selfhosted` with the reason (the job is `push` to `lane/**` only, never `pull_request`, so on the path that actually gates `main` it is skipped and adds no signal; and on a lane push a queued self-hosted Mac would leave a REQUIRED context pending for the 24h GitHub takes to cancel it -- `always()` does not help, because a need that never resolves is never reached). The declaration cannot rot: the sibling assertion `no not-aggregated declaration names a job that no longer exists` grades it in the same run and passes. (4) `no run: block in the report job enumerates a dependency` -- the aggregate step was an inline `run:` naming all six dependencies beside a six-name `needs:` list, with nothing keeping the two in sync. FIXED: replaced by `bash .github/scripts/assert-no-dependency-failed.sh`. (5) `the aggregate gate is fed the WHOLE needs object` -- it was not fed one at all. FIXED: `env: NEEDS_JSON: ${{ toJSON(needs) }}`, so a dependency added to `needs:` cannot be forgotten by the gate. NOTE ON WHERE THE ONE GENUINELY STALE ASSERTION WAS: report-gate-wiring.test.sh carried a literal `want_grep` for `needs.ci-linux.result != 'cancelled'` -- it pinned the exact string fix (1) had to delete, so the two gates contradicted and the older literal one lost. #417 replaced it with a property assertion over the same requirement plus an anti-vacuity arm. That was in the sibling harness, NOT in gate-admission.py, and it is the reason c2 below exists. THE LEDGER'S PRIOR GRADE IS SUPERSEDED, NOT CONTRADICTED: the A/B recorded here in the old note was correct at be28da9c7 vs 852f5acaa; both predate the fix, and the row was never re-graded after #417 landed."
  - id: c2
    text: "The reconciliation with core#412 c2 is written down where both gates can be read together: which admission forms satisfy both rules, and which satisfy only one."
    state: met
    evidence: "file:.github/scripts/tests/gate-admission.py:438:RECONCILIATION with core#412 c2"
    owner: core
    note: "WAS GENUINELY ABSENT AT 509f4426b AND IS NOW MET AT 757dfb91b. Verified absent rather than assumed: at the base commit `grep -rn check-ci-step-suppression` over the repo returned only ci.yml and .planning/ledger/wayland-core-412.md -- gate-admission.py never named that file and scripts/check-ci-step-suppression.py never named gate-admission.py, so neither half of the pair could be found from the other. THE DISAGREEMENT, STATED: gate-admission.py grades a step's condition by EQUALITY against the two-element set {always(), !cancelled()}; check-ci-step-suppression.py grades by SUBSTRING against the same two tokens. FORM BY FORM: `${{ !cancelled() }}` and `${{ always() }}` satisfy BOTH and are the only forms that do; `${{ !cancelled() && steps.ci_image.outcome == 'success' }}` satisfies core#412 ONLY (admitted there by substring, because a step running inside the CI image cannot report once the image build failed; rejected here by equality, because `always() && X` is a gate X can switch back off); no `if:` at all, `success()`, `failure()`, `needs.X.result != '...'` and `hashFiles(...) != ''` satisfy NEITHER. NOTHING satisfies gate-admission.py alone -- its accepted set is a strict SUBSET of core#412's, so the stricter rule is always the one to write to. Also recorded: the two rules reach DISJOINT jobs on this tree (gate-admission's prerequisite rule reaches ci.yml/report and e2e.yml/e2e_report, the only two jobs with an unconditional job-level `if:` that also run a gate script -- measured, not assumed -- while core#412's reaches ci.yml/ci-linux, which has no job-level `if:` at all), and that the escape hatches differ (core#412 has SUPPRESSIBLE; gate-admission has none before its last gate, deliberately). WRITTEN IN BOTH PLACES, and kept alive rather than left as prose: gate-admission.py carries the full form-by-form block, scripts/check-ci-step-suppression.py carries a pointer to it at NON_SUPPRESSING, and a new assertion `the two admission rules record their reconciliation and name each other` reds if the block is deleted, if the back-pointer is removed, or if the other file disappears. All three RED ARMS RUN: delete-the-block -> FAIL 'this file no longer carries the reconciliation block' RC=1; delete-the-back-pointer -> FAIL '...no longer points back at this file' RC=1; delete-the-file -> FAIL '...is gone' RC=1; unmutated -> PASS RC=0. The assertion's marker is SPLIT in source (`'RECON' + 'CILIATION with core#412 c2'`) so it cannot be satisfied by its own text -- the vacuity a self-referential check invites. The block is appended AFTER the existing assertions on purpose: .planning/ledger/wayland-core-405.md anchors gate-admission.py:288 with a +/-20 line window, and an insertion above it would have moved another lane's anchor."
  - id: c3
    text: "gate-admission.py exits 0 on the shipping branch, PROVEN by a run in the fmt + clippy job, and driven RED by re-introducing one of the five so the green is not the green of a disabled check."
    state: met
    evidence: "file:.github/workflows/lint.yml:129:bash .github/scripts/tests/report-gate-wiring.test.sh"
    owner: core
    note: "MET, AND BOTH ARMS ARE REAL CI RUNS RATHER THAN LOCAL ONES. GREEN ARM, the criterion's own demand: run 33828610484, push, `main` @ 509f4426b, 2026-09-04T02:11:52Z -- job `fmt + clippy (workspace, all targets)`, step `Self-test the CI evidence gates`, conclusion SUCCESS. That step is lint.yml:129 `bash .github/scripts/tests/report-gate-wiring.test.sh`, which invokes gate-admission.py and re-emits every one of its PASS/FAIL lines, so a green there is the gate exiting 0 and not the gate being absent. The harness also carries its own anti-vacuity arm (`the admission sweep reported its assertions`, requiring >= 10 lines) so a sweep that printed nothing cannot read as a sweep that passed. RED ARM, ALSO A REAL RUN, and it is the same five: run 33527830042, integ/f13 @ ea4d4dbb, same job, log ends `passed: 44  failed: 5` with `FAIL every caller of an unconditional gate...`, `FAIL in an unconditional job...`, `FAIL every ci.yml job is aggregated...`, `FAIL no run: block in the report job enumerates a dependency`, `FAIL the aggregate gate is fed the WHOLE needs object`. So this required-adjacent job has been observed BOTH red for exactly this defect and green after it was fixed -- the green is not the green of a disabled check. RE-CONFIRMED LOCALLY on the lane tree at 757dfb91b: gate-admission.py 25 PASS / 0 FAIL RC=0, report-gate-wiring.test.sh `passed: 52  failed: 0` RC=0, and scripts/check-ci-step-suppression.py --self-test `both directions proven` RC=0 with 30 steps graded. RE-INTRODUCTION ARM RUN AGAIN ON THIS TREE, with the gate held byte-identical: current gate + 852f5acaa's ci.yml -> RC=1, 5 failures; current gate + current ci.yml -> RC=0. THE ONE THING THIS ROW DOES NOT CLAIM: run 33828610484 predates 757dfb91b, so it proves the 24 assertions that existed at 509f4426b, not the 25th added by this lane. That one is proven locally only, in both directions, and will be covered by CI on the first lint run of a tree containing 757dfb91b."
---

Filed 2026-08-31 while discharging core#412 c2/c3. The `fmt + clippy (workspace,
all targets)` job is red on `integ/f13` for this reason alone, independently of
what the code does.

0.13.13 rather than 0.13.12: it is a gate that is stuck red, not a gate that is
falsely green, so it degrades signal without certifying anything untrue. core#412
was the falsely-silent case and that one blocked.

# Re-graded 2026-09-04: the defect was fixed in 0.13.12 and never re-read

The five failures were real, and every one of them was a real admission defect in
`ci.yml` rather than a stale expectation in the gate. They were fixed at
93ede3424 -- the 0.13.12 merge, which is the same commit that last touched
`gate-admission.py`, and which also ADDED a sixth rule after a red it caught
(`a step that runs a repo script comes after the checkout`, PR #417 run
33542986300, where the aggregate gate had been rewritten into a script
invocation and left above `actions/checkout`).

What was left undone is the part that has no test to force it: the two gates that
now hold rules about the same fact still did not know about each other. That is
c2, and it is the drift that already cost this repo once -- `report-gate-wiring.test.sh`
pinned the literal string `needs.ci-linux.result != 'cancelled'` that
`gate-admission.py` had just made illegal, so satisfying either gate reddened the
other. It is closed here by writing the agreement in both files and grading it.

`status: open` deliberately: closing is a release action, not a lane action.

READ THIS BEFORE TREATING THE LEDGER GATE'S RED AS A DEFECT. With all three
criteria honestly `met` and FerroxLabs/wayland-core#414 still open on GitHub,
`scripts/check-criteria-ledger.py` reports exactly one problem, by design:

    DIVERGENCE: .planning/ledger/wayland-core-414.md marks every criterion met,
    but FerroxLabs/wayland-core#414 is still open. Either the issue closes or a
    criterion is not actually met.

That is the checker doing its job, and there is no ledger-side spelling that
clears it: setting `status: closed` while the issue is open trips the OTHER
divergence arm (`says status: closed; ... is open on GitHub`), and marking a
criterion not-met to buy a green would be manufacturing a failure. CONTROL, same
checker, same tree, one session: with this file at its ungraded state the run is
`OK: every ledger file parses...` RC=0, and with it graded it is `FAIL: 1
problem(s)` RC=1 with only the line above. The gate goes green the moment #414 is
closed, which is the handoff this row is waiting on.
