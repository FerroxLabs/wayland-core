---
issue: 405
repo: FerroxLabs/wayland-core
kind: defect
title: "build-darwin-selfhosted is a ci.yml job the required report check never aggregates"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The disposition is DECIDED and recorded: either report gains build-darwin-selfhosted in its needs: list, or it does not and the reason is written AT the needs: list where the next reader looks, not only in this ticket."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "A check proves no OTHER ci.yml job is missing from report's needs:, so the one gap that was noticed does not hide a second. One unaggregated job was found by reading; the question is whether reading found them all."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "Whichever way c1 goes, the cost is named: on lane/** an offline self-hosted Mac would redden every lane push, and on main and PRs the job does not run at all so aggregating it changes nothing there."
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

c1 is a maintainer call by the issue's own account -- neither reason is
decisive without one. It is written defect rather than task because c2 is code
and is gradeable by a lane today; if c1 is answered and c2 is all that remains,
this should be re-kinded or decomposed rather than left mixed.
