---
issue: 367
repo: FerroxLabs/wayland-core
kind: defect
title: "A never-merge red-arm instrument reached integ/f13: OwnedTree owns the leaf only again on Unix"
status: open
last_verified_commit: b5df7167
criteria:
  - id: c1
    text: "The leaf-only instrument is out of the tree, and the guard's own regression test proves it"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees.rs::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child"
    owner: core
    note: "The ten lines 8d6add71 added to crates/wcore-cli/tests/support/owned_tree.rs are removed on lane/f13-fin-handoff-audit. This criterion cites the regression test rather than the source because the test is what makes the removal checkable: with the instrument present it fails 3 of 3 in a workspace run AND 3 of 3 alone on an otherwise idle box -- verbatim, `the grandchild 312060 outlived the guard - killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)` at harness_owns_spawned_trees.rs:121:5 -- and with it removed the binary is 24/24 green. That IS the red arm; it did not have to be constructed, it was running in the integration branch."
  - id: c2
    text: "An integration run's failing-test SET is compared against a named allow-list, so `1 failed` can never be read as `the known 1 failed`"
    state: not-met
    owner: core
    note: "This is the defect that matters and it is not the ten lines. A workspace nextest on integ/f13 reported exactly one failure, this repo has one standing known failure (wcore-exec-backend::conformance_matrix::every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not), and the two are indistinguishable from a count. Three commits landed on top of the merge before anyone opened the name. scripts/flake-ledger.py is not this: it re-measures a NAMED set at retries=0 and answers whether a failure is load-dependent, which is a different question from whether the failing set is the expected one. NOT proposed, and the ticket's original suggestion is withdrawn: a grep for `black_box(true)` or a `RED ARM` comment. `RED ARM` is a legitimate doc-comment idiom on dozens of real tests here, and the next instrument spells itself `cfg!(all())` or a `const` -- that is the game of spellings Q2 in .planning/DECISIONS.md already refused once, and a half-guard buys false coverage, which is worse than a documented gap. Compare the SET, which is exact and has no spellings."
---

Found by the 0.13.12 handoff audit, not by the lane that produced it — which is
the whole point of the second criterion.

`8d6add71` says **"RED ARM (throwaway, never merge)"** in its own subject line
and **"Delete this branch after reading the run; it is an instrument, not a
fix"** in its body. `d03a6e14` merged it into `integ/f13`. Its content reduces
`OwnedTree::snapshot` to leaf-only ownership behind `std::hint::black_box(true)`
— chosen so the dead code below stays reachable to the compiler and `clippy -D
warnings` still passes, which is a reasonable thing for an instrument to do and
is exactly why the static gates could not see it.

The effect is `wayland#1156` reintroduced: the guard owns the direct child and
not the tree, so a backgrounded grandchild survives.

c1 is done. c2 is the reason this is filed as a defect rather than closed with
the revert: the tree was wrong for four commits, a full test run said so every
time, and the signal was a number nobody expanded.
