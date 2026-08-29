---
issue: 1181
repo: FerroxLabs/wayland
title: "Four orphaned lane branches carry unmerged fixes, two of them in the 'a check that ran nothing' class"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "lane/walk-parallel has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "graded the serial secret-deny walk twice instead of the parallel one, so its assertion could not fail for the thing it named"
  - id: c2
    text: "lane/winpath has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
  - id: c3
    text: "lane/tools-bash has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
  - id: c4
    text: "lane/win-fix has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: not-met
    owner: core
    note: "its subject is CI ran zero tests again — a green check that ran nothing, the class this repo treats as most serious"
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
