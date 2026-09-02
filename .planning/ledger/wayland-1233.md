---
issue: 1233
repo: FerroxLabs/wayland
kind: defect
title: "Eight helper-attributed env-global hazards, now audited and carried as dated debt"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Each of the eight pairs in .config/env-global-helper-debt.txt reaches a terminal state: the helper stops writing the process global (the value is stated at the call site, the shape ContainerBackend::with_image already used), or the pair is serialized, or the entry is re-dated with a measured reason. None is left listed with nobody having looked at it."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "The three temp_state() rows are fixed as ONE helper duplicated across three integration targets, not as three independent fixes, so the duplication does not regrow. This is the same defect as wayland#1250 and the two close together or the overlap is stated."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "The wcore-cli row is treated as the production finding the table says it is -- run_gateway is production code reached from an unserialized test -- so the fix lands in the gateway, or the reason it lands in the test instead is recorded."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "What happens when a dated debt entry passes its date is DECIDED and enforced: either the gate fails on an expired entry, or the absence of an expiry is recorded as deliberate. A debt file whose dates carry no consequence is a list, not debt."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c5
    text: "These are invisible under nextest by construction (one process per test) and only observable on the shared-process legs. Whatever closes this is graded on a shared-process run, not a nextest one."
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

The eight are real hazards only where one test binary is one process. That is
the whole of wayland#1134: the main CI legs run nextest, which gives every test
its own process, so none of these can ever be observed there. Grading a fix on
a nextest run would be a green from the wrong instrument.
