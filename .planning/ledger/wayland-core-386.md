---
issue: 386
repo: FerroxLabs/wayland-core
kind: defect
title: "core#325 c2 remainder: one real nightly-windows-soak run with a red sibling"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "One real nightly-windows-soak.yml run with a genuinely red sibling job is shown to NOT close the tracker issue -- observed on GitHub, against the live Octokit, not against the stub."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "That same run posts a comment on the tracker naming windows-live-acceptance, and opens one if none is open."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "If a real red sibling cannot be produced on demand, that is RECORDED as the ceiling and the stubbed-Octokit evidence is stated as what it is. The 30 assertions and three red arms already committed are strong, and they are not narrated as equivalent to a live run."
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

Everything executable off GitHub is already done: the test drives the real
workflow, the real interpolation, the real decision script and the real
github-script bodies under node against a stubbed Octokit. What remains is
the one thing a stub cannot supply, which is why it is a remainder and not a
restatement of core#325 c2.

This sits close to kind: task -- it needs a real red job on a real platform.
It is written defect because c3 is code and judgement, not a credential a
human obtains, and because the gate's own rule is that an ambiguous entry is
written defect: over-blocking costs a conversation, under-blocking ships.
