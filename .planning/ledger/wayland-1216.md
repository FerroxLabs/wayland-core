---
issue: 1216
repo: FerroxLabs/wayland
kind: defect
title: "The report job's evidence floor cannot notice that the leg running the whole workspace suite contributed nothing"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The evidence floor is per-leg: a leg that uploads zero junit files fails the required report check rather than being covered by another leg's upload"
    state: met
    evidence: 'file:.github/scripts/assert-test-evidence.sh:105:REQUIRED_LEGS="${REQUIRED_LEGS:-}"'
    owner: core
    note: "MET AS WRITTEN. `assert-test-evidence.sh` now takes REQUIRED_LEGS -- one line of `<artifact-subdirectory> <job> <job-result>` -- and fails, per leg, when a named leg contributed no report or no test case, whatever any other leg uploaded. ci.yml names `nextest-junit-linux-containerized ci-linux` (the workspace-suite leg, unconditional, and the only leg carrying that coverage) and the step's `if:` was widened to fire on ci-linux's own result so the floor is reachable on lane pushes where `ci` is skipped. A leg whose job was cancelled or skipped is not required to report, so a conditioned platform cannot make this permanently red. DEFECT FOUND AND FIXED INSIDE THIS LANE: the headline case -- the leg uploaded NOTHING, so download-artifact created no subdirectory -- exited 1 through `set -e` aborting on `find` over a missing path, BEFORE the annotation was written: right exit code, no leg named, and it would have evaporated if anyone relaxed `set -e`. The absent directory is now read as zero and falls through to the named failure. COMPLETENESS: the gate has exactly TWO workflow callers -- ci.yml:2845 and e2e.yml:254 -- enumerated by grep over .github/. e2e.yml needs no REQUIRED_LEGS: each of its legs uploads exactly one path (target/nextest/e2e/junit.xml, no outer-attempts), so a leg can contribute at most one file and its computed EXPECTED_MIN of 1-or-2 already equals the number of legs in scope. That is a count standing in for a name and it holds only while that upload stays single-path; it is stated here rather than assumed."
  - id: c2
    text: "Preserved outer-attempt-*.xml files are not counted toward the coverage figure they are meant to prove"
    state: met
    evidence: 'file:.github/scripts/assert-test-evidence.sh:61:FOUND=$(find "$EVIDENCE_DIR" -type f -name "*.xml" ! -name "outer-attempt-*.xml" | sort)'
    owner: core
    note: "MET AS WRITTEN. `outer-attempt-*.xml` is excluded from BOTH aggregate counters (the report count and the test-case count) and from the per-leg counters. Those files are the JUnit of an attempt an outer retry loop preserved (#1177); grade-retry-flakes.sh owns them and grade-failing-set.sh already skips them, so counting them here let a leg's FAILURES stand in for the coverage they are supposed to prove. RED ARM, and a second defect it exposed: deleting the exclusion from the `FOUND=` line alone did NOT red the suite, because the pre-existing arm reds through MIN_TESTS instead -- the file-count exclusion was untested. A new arm holds EXPECTED_MIN at 2 over one real report beside one preserved attempt (e2e.yml computes EXPECTED_MIN from scope and routinely asks for more than one), with a two-real-reports control at the same floor. With the exclusion deleted that arm now reds; restored, the suite is 50/0."
  - id: c3
    text: "A test under .github/scripts/tests/ drives both directions and is wired into lint.yml so a failure reds the step"
    state: met
    evidence: 'file:.github/scripts/tests/assert-test-evidence.test.sh:441:leg_says "absent leg dir NAMES the leg, not a bare set -e abort" 1'
    owner: core
    note: "MET AS WRITTEN. `.github/scripts/tests/assert-test-evidence.test.sh` is invoked by .github/workflows/lint.yml:127 (report-gate-wiring.test.sh at :129) with no `continue-on-error`, so a failure reds the step. Both directions on every case: silent leg RED / same leg reporting GREEN; zero-test junit RED; preserved-attempt-only RED / preserved attempt beside a real report GREEN; skipped and cancelled legs GREEN, the same leg with result `failure` still RED. The arm that matters most asserts the ANNOTATION TEXT and not just the exit code, because the exit code alone was satisfied by a bare `set -e` abort -- with the pre-fix script that arm FAILS while the older exit-code-only arm stays GREEN, which is the substituted-property demonstration in miniature. Suite: 50 passed, 0 failed (exit 0); report-gate-wiring 32/0."
---

The report job's evidence floor cannot notice that the Linux containerized leg contributed nothing. `assert-test-evidence.sh` is invoked with `EXPECTED_MIN: 1` across ALL legs aggregated into `junit-reports/`, so if any single leg uploads a junit.xml the gate is satisfied even when the leg that runs the full workspace suite uploaded zero files (`if-no-files-found: ignore` at ci.yml:2027 makes that silent). Separately, the preserved `outer-attempt-*.xml` files now also land inside `junit-reports/` and are counted by the same `find ... -name '*.xml'` COUNT, so a leg's preserved failures inflate the number that is supposed to prove coverage.

**Where.** .github/workflows/ci.yml:2624-2635 (EXPECTED_MIN: 1) and :2015-2027 (upload with if-no-files-found: ignore); counting logic in .github/scripts/assert-test-evidence.sh:55-67.

**Why it matters.** It is the same defect class the report gate was built for (wayland#1115: a green `report` on a suite that never ran), one level finer — per-leg rather than repo-wide. The ci.yml comment at :2620-2623 records the decision to keep EXPECTED_MIN at 1 deliberately, so this is a known trade rather than an oversight, but defect 1 above is a live instance of the leg it cannot see. Today the `ci` job is red on its own account so no run is green on this, but the gate contributes nothing to catching it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
