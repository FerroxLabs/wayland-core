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
    evidence: "test:crates/wcore-protocol/tests/contract_gate_topology.rs::the_outer_retry_evidence_tree_is_reserved_before_any_container_mounts_the_workspace"
    owner: core
    note: "THE PRESERVATION WAS CORRECT AND NEVER RAN. On run 33227927478 job 99035159787 the wrapper died on BOTH attempts at `mkdir -p $ATTEMPT_DIR` with `Permission denied` before invoking nextest -- no test ran, no junit.xml existed, and the required report check received zero evidence from the leg that carries the whole workspace suite. Cause: $DOCKER_RUN has no `-u`, so a root container step creates target/ while the wrapper runs on the host as uid 1001. The first repair (e7144c30) added a bare `mkdir -p` AFTER the corpus pre-flight hint -- itself a `docker run ... cargo run` -- so on every pull_request run it died with the identical error one step earlier, and the self-test graded it with a grep for the mkdir string, which a wrongly-ordered step satisfies exactly as well as a correct one. REPRODUCED outside CI 2026-08-29: root container creates target/, uid 1001 gets `mkdir: cannot create directory 'target/nextest': Permission denied`. Now: the step runs FIRST in the job and goes through reserve-attempt-evidence-tree.sh, which repairs the owner if the ordering is ever wrong again (verified in a uid-1001-with-sudo container: bare mkdir exit 1, script exit 0, tree writable, junit removable); the ORDERING is asserted by the named test, which fails on a clean checkout of the pre-fix ci.yml."
  - id: c2
    text: "A test demonstrates that visibility and fails against today's workflow"
    state: met
    evidence: "file:.github/scripts/tests/outer-retry-evidence.test.sh:501:THE ASK: attempt-1 failure retried green is visible to the report check"
    owner: core
    note: "PART E replaces the grep. The wrapper is RUN twice against a stub that fails then passes, its outputs are assembled into the exact layout download-artifact produces from ci.yml's two upload paths, and the REAL assert-test-evidence.sh -- the script the required report step invokes -- is then run over it with that step's own environment; it exits 1 and names probe::races_under_load. Anti-vacuity: the identical pipeline for a leg that passed first time exits 0. The earlier version was 19/19 green on a tree where the mechanism was 100% inoperative on the runner, which is what a piecewise grade buys. Two further arms close the report gate's own blind spot (D34): report now `needs: ci-linux` -- it did not, and the ci matrix has no Linux entry, so it never even waited for the leg that runs the workspace suite -- and REQUIRE_LEGS gives the aggregate EXPECTED_MIN a per-leg floor, with preserved outer-attempt-*.xml excluded from the coverage count so a leg's failures cannot stand in for its coverage. 40/40 green; 30/40 on a clean checkout of the pre-fix scripts."
  - id: c3
    text: "The #1169 retry-flake grader keeps working on the nextest layer it already covers"
    state: met
    evidence: "file:.github/scripts/grade-retry-flakes.sh:119:-print0 2>/dev/null |"
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
