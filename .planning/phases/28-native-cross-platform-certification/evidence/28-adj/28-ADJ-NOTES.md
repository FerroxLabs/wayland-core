# 28-ADJ NOTES — independent adjudication of F-28-02-002

Lane: `lane/28-adj`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-28-adj`.
Base: b79f141e (plan/f20-unified-audit-repair tip at branch time).

## Question
Does the FIXED claim for F-28-02-002 (stale AppContainer lease wedge, HIGH, persistent DoS)
survive an adversarial independent pass? Only FIXED or DISPROVED are available dispositions.

## Status log
- [t0] Worktree created. Read LANE-BRIEF, 28-H2-SUMMARY.
- 28-H2-SUMMARY claims: repro on real Windows hw at 12fc794f; repair; both legs re-measured at
  3f3f93dc; 133 passed/0 failed/23 ignored unit; M1/M2 mutants each kill exactly one test;
  live acceptance 20 passed 3 failed (3 pre-existing bwrap-on-Windows, identical at base).
- 28-H2-SUMMARY §8 explicitly says fix is on lane/28-h2 ONLY, not merged. Brief to me says
  "the repair merged". MUST VERIFY whether source actually landed on the integration branch,
  or whether only the docs commit (166ce7fe) landed. If only docs landed, FIXED cannot be
  written into the ledger — that would be a paper disposition.

## Open attack lines
1. Is the repair source actually present on plan/f20-unified-audit-repair?
2. Does the quarantine allow-list create a new wedge / writable surface / trust crossing?
3. Is the honour-when-alive leg real (not reclaim-everything)?
4. Fourth self-passing gate the lane did not catch (assume it exists).
5. Is the KR-05 non-closure scope statement accurate?
6. Gate self-test at 28-04-FINDING-LEDGER.md:1182 expects exactly one F28L-002 on F-28-02-002.
   Moving the row without moving the expectation breaks or vacates the gate.
