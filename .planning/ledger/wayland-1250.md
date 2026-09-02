---
issue: 1250
repo: FerroxLabs/wayland
kind: defect
title: "wcore-exec-backend tests race on the WAYLAND_EXEC_BACKEND_STATE_DIR process global in the shared-process suite"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "temp_state() stops writing the process global: the state directory is passed to the constructor, the shape ContainerBackend::with_image already used for WAYLAND_EXEC_CONTAINER_IMAGE."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "The fix covers all FOUR test binaries that set the var, not only conformance_matrix.rs, which is the one that reddened CI."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "Shown RED: the interleaving reproduces on a shared-process run before the fix, with the 1 passed / 1 failed signature quoted, and does not after. Isolation passes 8/8 today and so proves nothing either way."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "The three temp_state() rows carried as dated debt in wayland#1233 are REMOVED from .config/env-global-helper-debt.txt by this fix rather than left listed against a helper that no longer writes a global."
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

1 passed; 1 failed out of the two tests in that binary is the signature of the
two racing each other, not of a single broken test.
