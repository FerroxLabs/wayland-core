---
issue: 1181
repo: FerroxLabs/wayland
kind: defect
title: "Four orphaned lane branches carry unmerged fixes, two of them in the 'a check that ran nothing' class"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "lane/walk-parallel has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "lane/walk-parallel is 13a81ab8 and is NOT an ancestor of the graded tree. No rebase-merge, no named superseding commit, no obsolete verdict in git log, .planning/ or docs/. The only in-tree mention is MASTER-PLAN.md:242, which restates the problem and prescribes an outcome rather than recording one. CORRECTION to the previous note: all four tips DO resolve as local branches in this worktree - the earlier claim that they do not was wrong."
  - id: c2
    text: "lane/winpath has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "lane/winpath is 4089798c (fix(skills): normalize the output-dir token to forward slashes, 2026-08-22), NOT an ancestor. No disposition recorded anywhere."
  - id: c3
    text: "lane/tools-bash has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "lane/tools-bash is c7aeaf2d (fix(tools): name the cause when the manifest build times out, 2026-08-22), NOT an ancestor. No disposition recorded anywhere."
  - id: c4
    text: "lane/win-fix has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "lane/win-fix is c5ce3857 (fix(windows): CI ran zero tests again, 2026-08-01), NOT an ancestor and 28 days stale. .planning/WINDOWS-TRIAGE-2026-07-31.md:5 cites the branch as a source but records no merge, supersede or obsolete verdict."
  - id: c5
    text: "lane/finish-a and lane/finish-b, named in the issue as unpushed branches that would orphan on a box loss, have landed"
    state: met
    evidence: "commit:d92e61d1"
    owner: core
    note: "Added 2026-08-29; the issue's trailing ask had no criterion. d92e61d1 merges lane/finish-a and 883b2504 merges lane/finish-b into integ/next, so the box-loss risk is discharged. NOTE the same class is live again: lane/session-tickets is twelve commits ahead of integ/next and unmerged."
---

Four `lane/*` branches are not ancestors of main and were never merged or closed
out. Each carries a real fix, and two of them fix failure classes this repo
treats as worst: an assertion that could not fail for the thing it named, and a
green check that ran nothing.

They are 6 to 27 days stale against bases that have moved a long way, so merging
them unverified would be the same mistake in the other direction. Each needs
three answers before it moves: does the defect still exist on current main, is
the fix still correct against the current tree, and does its test still go red
under the mutation it was written for.

One criterion per branch, because the acceptance is explicitly per branch and
three outcomes are allowed for each. None of the four tips resolves in this
worktree's remotes, so nothing here is evidence that any of them has been
handled — only that this checkout cannot see them.
