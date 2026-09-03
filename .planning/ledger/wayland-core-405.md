---
issue: 405
repo: FerroxLabs/wayland-core
kind: defect
title: "build-darwin-selfhosted is a ci.yml job the required report check never aggregates"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "The disposition is DECIDED and recorded: either report gains build-darwin-selfhosted in its needs: list, or it does not and the reason is written AT the needs: list where the next reader looks, not only in this ticket."
    state: met
    evidence: "file:.github/workflows/ci.yml:2892:# not-aggregated: build-darwin-selfhosted"
    owner: core
    note: "MET at 6e4eca07, landed by 93ede3424 -- WHICH IS THIS ROW'S OWN PREVIOUS last_verified_commit, so the row was authored against a tree that already carried the fix and was never re-graded. Disposition DECIDED: NOT aggregated. The declaration is ci.yml:2892 and the reason sits at the needs: list it governs (ci.yml:2894-2908, needs: at ci.yml:2909), not only in this ticket. ORIGINAL: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "A check proves no OTHER ci.yml job is missing from report's needs:, so the one gap that was noticed does not hide a second. One unaggregated job was found by reading; the question is whether reading found them all."
    state: met
    evidence: "file:.github/scripts/tests/gate-admission.py:288:every ci.yml job is aggregated by `report` or declared not-aggregated"
    owner: core
    note: "MET at 6e4eca07. gate-admission.py:283-296 diffs the parsed ci.yml job roster against report.needs plus every `# not-aggregated:` declaration, and also reds a declaration naming a job that no longer exists. RUN on this tree: 22 PASS / 0 FAIL, exit 0, `INFO ci.yml jobs=7 aggregated=6 declared-not-aggregated=1`. Independent enumeration agrees: roster minus needs == [build-darwin-selfhosted] and nothing else, so reading DID find them all. RED ARMS on a scratch copy (the shared tree was never written): delete the declaration -> FAIL unaccounted; rename it to a job that does not exist -> FAIL unaccounted plus FAIL stale declarations; drop ci-linux from needs -> FAIL unaccounted: [ci-linux]. Wired at lint.yml:125-129 via report-gate-wiring.test.sh:68,74. RESIDUAL, not closed here: that job (`fmt + clippy (workspace, all targets)`) is NOT in main's required contexts, so the sweep reds a non-blocking check; and gate-admission.py:283 captures only the job NAME, so it does not enforce that a future declaration carry a reason. ORIGINAL: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "Whichever way c1 goes, the cost is named: on lane/** an offline self-hosted Mac would redden every lane push, and on main and PRs the job does not run at all so aggregating it changes nothing there."
    state: met
    evidence: "file:.github/workflows/ci.yml:2902:the job sits QUEUED and GitHub only cancels it after 24h"
    owner: core
    note: "MET at 6e4eca07, and the tree states the cost MORE precisely than the criterion did. The criterion said an offline Mac would "redden" every lane push; ci.yml:2900-2905 records the measured mechanism instead -- the job sits QUEUED, GitHub cancels it only after 24h, and `if: always()` cannot rescue it because it decides admission once needs RESOLVE and a queued need never resolves, so `report` (a required context) would PEND for a day rather than go red. The other half is at ci.yml:2895-2898: on main and on every PR the job's `if:` is false and it is SKIPPED, so aggregating it changes nothing on the path that gates main. Corollary from wayland-1291's recorded residual: the aggregate treats `skipped` as OK, so even aggregated it would contribute nothing there. ORIGINAL: AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
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
