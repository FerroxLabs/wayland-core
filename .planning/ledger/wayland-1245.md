---
issue: 1245
repo: FerroxLabs/wayland
kind: defect
title: "Flaky: t19_live_negative_leg has a 45s drain window with zero headroom; it fails 3/3 above loadavg ~150"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "The zero-headroom construction is removed: the negative leg concludes absence on a signal that is not a fixed wall-clock window, or the window carries headroom measured against the observed completion, which lands at 45.5 s against a 45.0 s deadline even when it passes."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "Shown RED against today's code at a load where it fails today -- loadavg above roughly 150 on hetzner-dsm, where it failed all three --profile ci attempts -- and green after, at the same load."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "The rate is recorded WITH the load figure beside it. A rate with no load number cannot separate this defect from ambient noise, and the whole finding is load-conditioned."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "The positive leg stays fast: the early return on the sentinel that keeps it fast is not removed to fix the negative one."
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

Both green runs land at 45.5 s against a 45.0 s deadline, so the assertions are
satisfied in the drain loop AFTER the window closes. The test is already over
its own budget on the runs that pass, which is why the pass is not evidence.
