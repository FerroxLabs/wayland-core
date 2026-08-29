---
issue: 1177
repo: FerroxLabs/wayland
kind: defect
title: "An outer workflow retry overwrites junit.xml, erasing a genuine failure before any grader sees it"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A failure on attempt 1 followed by a pass on attempt 2 leaves evidence the required report check can read"
    state: met
    evidence: "file:.github/scripts/run-tests-with-attempt-evidence.sh:90"
    owner: core
    note: "Each outer attempt's junit.xml is copied to outer-attempt-<n>.xml; wired at ci.yml:1657 under nick-fields/retry max_attempts: 2, uploaded at ci.yml:2005-2007 and read by grade-retry-flakes.sh:168 inside the required report job. The outer retry was PRESERVED, not dropped: the script also removes the stale report before each attempt and writes final-status.txt so 'retried into green' is distinguishable from 'red on the final attempt'."
  - id: c2
    text: "A test demonstrates that visibility and fails against today's workflow"
    state: met
    evidence: "file:.github/scripts/tests/outer-retry-evidence.test.sh:84"
    owner: core
    note: "Three parts: grade-retry-flakes.sh on fixture evidence, the writer against a stub that fails then passes, and a grep of the ci.yml wiring. Two negative controls (a clean suite stays green; an ordinary red is not re-reported) plus the agent-crash shape where no JUnit is written. It grades the scripts and the wiring, not a live CI replay."
  - id: c3
    text: "The #1169 retry-flake grader keeps working on the nextest layer it already covers"
    state: met
    evidence: "file:.github/scripts/grade-retry-flakes.sh:119"
    owner: core
    note: "The #1169 nextest-layer scan is intact at :119; the outer-attempt scan at :168 is an ADDITIONAL pass keyed on outer-attempt-*.xml plus final-status.txt, with an absent status graded fail-closed."
---

`.github/workflows/ci.yml` wraps the containerized Linux workspace test run in
`nick-fields/retry@v3` with `max_attempts: 2`. The second attempt overwrites
`target/nextest/ci/junit.xml`, so a test that genuinely failed on attempt 1 and
passed on attempt 2 leaves no structured trace at all. It is not even recorded
as a flake, because the flake record lived in the file that was just
overwritten.

This is the same defect as #1169 one layer up, and strictly worse. #1169's
grader runs inside the required `report` job and reads that XML — so it cannot
see this layer, because the outer retry destroys the evidence before the grader
runs. The fix for #1169 is real and incomplete.

Nothing has been done. The issue also records a practical obstacle for whoever
picks it up: editing `ci.yml` needs a token with `workflow` scope, which the
hetzner build box does not have. That is a routing detail, not a blocker on
another lane, so this stays core's and not-met rather than blocked.
