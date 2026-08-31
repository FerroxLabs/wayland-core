---
issue: 1235
repo: FerroxLabs/wayland
kind: defect
title: "One mock turn with a 480 KB tool result costs 63.6 s of CPU; the spill/read-back test is killed by the default nextest budget 3/3"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "The 63.6 s of USER CPU is ATTRIBUTED to a named function on the turn loop by measurement -- a profiler or bisecting instrumentation inside the turn path -- and not inferred from reading. 0.10 s system and 38 MB RSS already rule out disk and swap, so the remaining question is which computation, not whether."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "Whether the cost is superlinear in payload size is SETTLED with measurements at three or more payload sizes, since the body records that the cost is not proportional to the payload. The exponent or the shape is stated as a number."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "Either the test completes inside [profile.default]'s 30 s slow-timeout, or it carries an explicit per-test budget in .config/nextest.toml with the reason recorded beside it."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "If c1 finds a product cost on the path a real user hits, a regression guard fails when the per-byte term grows. If c1 finds a fixture artifact, that is recorded where the next person measuring a slow wcore-agent test will find it."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c5
    text: "It is NOT closed by adding a line to .config/flaky-allowlist.txt. The issue is explicit that doing so would suppress the symptom of a product cost nobody has measured, and that refusal is the reason it is an issue at all."
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

Overlaps wayland-core#395, which asks the same debug-vs-release question about
the same per-byte term and carries handoff: FerroxLabs/wayland-core#378. Whoever
takes either should take both, or state which one owns the measurement.
