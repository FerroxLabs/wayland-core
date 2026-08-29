---
issue: 1220
repo: FerroxLabs/wayland
kind: defect
title: "A cleared flaky-allowlist entry came back through a merge, and nothing in this repo can detect a resurrected line"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: ".config/flaky-allowlist.txt on the integration branch no longer carries the gh#1182 line"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D40, found while verifying wayland#1182). Nothing has been done. The measured finding, verbatim: A flaky-allowlist entry that was deliberately deleted (commit c461293f, 'clear three fixed flaky-allowlist entries') was silently resurrected by merge commit 9c9f27b0 'Merge remote-tracking branch origin/lane/f13-fix-shared-lib into integ/f13'. `git diff 9c9f27b0^1 9c9f27b0 -- .config/flaky-allowlist.txt` shows the gh#1182 line coming back as a `+`. The other two entries the same commit cleared (dangerous_lease_e2e_test x2) did NOT come back, so this is a partial merge-resolution regression, not a wholesale revert."
  - id: c2
    text: "The allowlist is graded against the MERGED tree rather than against a commit hash: a check refuses an entry whose owning ledger criterion claims it was deleted"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D40). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The check is proven in both directions, including a resurrection introduced by a MERGE -- git log -S skips merges by default, which is how this one passed"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D40). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

A flaky-allowlist entry that was deliberately deleted (commit c461293f, 'clear three fixed flaky-allowlist entries') was silently resurrected by merge commit 9c9f27b0 'Merge remote-tracking branch origin/lane/f13-fix-shared-lib into integ/f13'. `git diff 9c9f27b0^1 9c9f27b0 -- .config/flaky-allowlist.txt` shows the gh#1182 line coming back as a `+`. The other two entries the same commit cleared (dangerous_lease_e2e_test x2) did NOT come back, so this is a partial merge-resolution regression, not a wholesale revert.

**Where.** .config/flaky-allowlist.txt:59 at origin/integ/f13; introduced by merge 9c9f27b0; original deletion in c461293f

**Why it matters.** The retry-flake gate (.github/scripts/grade-retry-flakes.sh, a REQUIRED context on main) now silently tolerates FLAKY retries of `wcore-tools::workspace_policy::tests::contained_construction_does_not_walk_the_workspace` until 2026-10-15. If that test starts flaking for a NEW reason — its own file explains it is a security-boundary instrument control — the run conclusion stays SUCCESS and nothing names it. More generally, nothing in the repo detects a resurrected allowlist line: `git log -S` skips merges by default, which is exactly how this passed the lane's own check. Every future 'cleared the allowlist entry' claim graded off a commit hash rather than the merged tree has the same hole.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
