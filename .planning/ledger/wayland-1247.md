---
issue: 1247
repo: FerroxLabs/wayland
kind: defect
title: "wcore-swarm worktree linux tests fail under full-workspace load, reddening ci-linux for unrelated lanes"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "Both named deadlines are addressed as a FAMILY: linux.rs:693 (read_child_pid's 3 s poll) and linux.rs:972. The issue found a second test on the first reproduction attempt, so fixing the one CI named would close the instance and leave the class."
    state: met
    evidence: "commit:2347d8f9c"
    owner: core
    note: "MET at 509f4426b. Both named deadlines were addressed in ONE change, which is what this criterion asks: 2347d8f9c fixes linux.rs:693 (`read_child_pid`, the 3s poll, now a 25s liveness backstop over a fixture record that carries its own terminator) and linux.rs:972 (`worktree_add_timeout_kills_tree_and_reports_preserved_residual`, stage pinned and budget re-derived) in the same commit. Anchored on the commit because the criterion property is that the two were treated as one family, which no single file token can express. VERIFIED IN THE TREE, not read off the message: `read_child_pid` at linux.rs:726 now uses `Duration::from_secs(25)`, and the config-stage pin is at linux.rs:1023-1032. NOT GRADED: c4 -- the family is NOT closed. `wait_until_process_gone` still carries a hard-coded `Duration::from_secs(3)` at linux.rs:638, so a grep over that file does not come back clean."
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
    state: met
    evidence: "file:crates/wcore-swarm/src/worktree_tests/linux.rs:708:refuses an unterminated record"
    owner: core
    note: "MET at 509f4426b. The polling mitigation is explicitly NOT what closed this. The fixtures now publish the pid with a shell builtin `echo`, which supplies its own newline, and `try_read_child_pid` REFUSES a record with no terminator (linux.rs:687 and the doc comment at :708), so a partial write can no longer be read as a short pid -- `1234` observed as `12` used to parse happily and name an unrelated process. The 3s budget was not merely widened either: the doc comment records that 25s was chosen to fit INSIDE nextest default 60s hard kill, measured both ways (at 60s the run reports only `TIMEOUT [60.005s]` with no diagnostic; at 25s it reports `FAIL [25.056s]` with the message that distinguishes a hang from a slow runner). WHAT WOULD FALSIFY THIS: the terminator refusal being removed, which the anchored line reds on."
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
