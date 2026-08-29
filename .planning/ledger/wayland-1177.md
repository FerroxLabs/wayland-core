---
issue: 1177
repo: FerroxLabs/wayland
title: "An outer workflow retry overwrites junit.xml, erasing a genuine failure before any grader sees it"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A failure on attempt 1 followed by a pass on attempt 2 leaves evidence the required report check can read"
    state: not-met
    owner: core
    note: "either attempt-scoped JUnit paths that the grader all reads, or dropping the outer retry for the already-graded nextest retries"
  - id: c2
    text: "A test demonstrates that visibility and fails against today's workflow"
    state: not-met
    owner: core
  - id: c3
    text: "The #1169 retry-flake grader keeps working on the nextest layer it already covers"
    state: not-met
    owner: core
    note: "grade-retry-flakes.sh reads the JUnit XML, so any change to how that file is produced is a change to what the gate can see"
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
