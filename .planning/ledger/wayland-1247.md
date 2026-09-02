---
issue: 1247
repo: FerroxLabs/wayland
kind: defect
title: "wcore-swarm worktree linux tests fail under full-workspace load, reddening ci-linux for unrelated lanes"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Both named deadlines are addressed as a FAMILY: linux.rs:693 (read_child_pid's 3 s poll) and linux.rs:972. The issue found a second test on the first reproduction attempt, so fixing the one CI named would close the instance and leave the class."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "linux.rs:972 is fixed at its cause -- the 200 ms git timeout fires at the git config safety check stage instead of the intended worktree add stage, so no residual path exists yet when the second assertion runs -- and not by widening the budget."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "The measured failure rate is re-measured after the fix at N of at least 13 on hetzner-dsm, the same N that produced the 1-in-13 baseline on a quiet host, and recorded."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "A grep or a test proves no other hard-coded short deadline remains in crates/wcore-swarm/src/worktree_tests/linux.rs, so the family is closed rather than the two instances that were noticed."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c5
    text: "The polling mitigation already applied to try_read_child_pid is not counted as the fix: its own doc comment records that it reduced the rate and the 3 s budget still loses under CI load."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

report is a required status context depending on ci-linux, so a red here fails
the required check for whatever lane happens to be pushing -- on a crate that
lane did not touch. That blast radius is the reason this is not a minor flake.
