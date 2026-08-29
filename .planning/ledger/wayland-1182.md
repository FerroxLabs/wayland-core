---
issue: 1182
repo: FerroxLabs/wayland
kind: defect
title: "The workspace-walk instrument check declares itself dead under load: a wall-clock ratio decides whether the control is alive"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Liveness of the workspace walk is established by a direct observation, not by comparing two wall-clock timings"
    state: met
    evidence: "symbol:crates/wcore-tools/src/workspace_policy.rs::walk_entries"
    owner: core
    note: "A thread-local WALK_ENTRIES counter incremented in both the serial arm (:2521) and the parallel arm (:2565), attributed to the thread that ASKED for the walk, so an oversized tree's serial prefix is not double-charged. No wall clock anywhere in the rewritten test."
  - id: c2
    text: "The test still goes red when the walk becomes unreachable, which is the property the control exists for"
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::contained_construction_does_not_walk_the_workspace"
    owner: core
    note: "The control was rewritten, not deleted: assert!(during_walk >= 3000) over a 3000-directory tree, so an unreachable walk reports 0 and still fails - the red arm is forced by a single readable assertion, not by timing. A second control asserts the walk still classifies (finds the planted .env), so 'enumerated it and understood nothing' cannot pass. The property itself is now assert_eq!(during_construct, 0), a count, removing the last per-platform timing constant."
  - id: c3
    text: "The test no longer needs its entry in .config/flaky-allowlist.txt under the #1169 retry-flake gate"
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::gh#1182"
    owner: core
    note: "RE-GRADED 2026-08-29 and REPAIRED. The previous evidence (commit:c461293f) was true and useless: that commit really did delete the line, and merge 9c9f27b0 (origin/lane/f13-fix-shared-lib into integ/f13) put it back from the other side of the resolution, so the entry was PRESENT at .config/flaky-allowlist.txt:59 while this criterion read met. The two dangerous_lease_e2e_test deletions from the same commit survived, so it was a partial merge regression, and `git log -S` does not show it because -S skips merges. Deleted again here; the allowlist is now 8 dated entries and gh#1182 appears 0 times, while the known-positive control grep -c crucible_council still returns 1. The evidence is now `absent:`, a kind added to scripts/check-criteria-ledger.py in the same change, which re-reads the file on EVERY gate run instead of pointing at a deletion that once happened -- a second resurrection reds the gate."
---

`contained_construction_does_not_walk_the_workspace` in
`crates/wcore-tools/src/workspace_policy/tests.rs` asserts that constructing a
contained workspace policy does not walk the tree. That claim passes for free if
the walk is unreachable, so the test guards its own instrument: it requires the
walk over a real tree to be measurably slower than the walk over an empty one.

The guard is a wall-clock ratio, and on a loaded 96-core box the two timings
compress until the control declares itself dead (`walk=20.4ms
walk_empty=8.2ms`). Workspace-walk cost here is genuinely load- and
cache-sensitive — 39,278ms cold versus 349ms warm has been measured on a real
tree — so timing is a fragile way to prove liveness on a shared machine.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has
been done: the test is allowlisted as a known flake with this issue as owner.
