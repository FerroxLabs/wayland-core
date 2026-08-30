---
issue: 1240
repo: FerroxLabs/wayland
kind: defect
title: "await_completion_returns_on_match reds the shared-process lib leg on a timing race, not a process global"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "The rate is MEASURED on the containerised CI image -- not on hetzner-dsm, whose four local passes the issue itself says do not exonerate the leg -- at --retries 0 over N of at least 20, and the rate is recorded."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "Either the test stops racing real elapsed time (assert a count, not a duration, the fix wayland#1182 applied to the workspace-walk control), or it is carried in .config/flaky-allowlist.txt WITH the rate c1 measured. That file's discipline is that an entry states what it measured, so an entry without c1 does not close this."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "A red arm is shown: after the change, the test still fails when the observer genuinely does not see Completed. A deadline made unreachable passes for the wrong reason."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "The required Shared-process lib suite check is left able to mean something: whatever closes this does not teach a reader to discount a red on that leg."
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

observer.rs:303 is assert!(matches!(got, Ok(AgentMessage::Completed { .. }))),
so the waiter one line above returned the timeout arm. Not contamination,
despite being surfaced by the leg built for that class.
