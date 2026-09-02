---
issue: 1182
repo: FerroxLabs/wayland
kind: defect
title: "The workspace-walk instrument check declares itself dead under load: a wall-clock ratio decides whether the control is alive"
status: closed
last_verified_commit: 93ede3424
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
    evidence: "absent:.config/flaky-allowlist.txt::contained_construction_does_not_walk_the_workspace"
    owner: core
    note: "RE-GRADED 2026-08-29 and REPAIRED, then NARROWED 2026-08-30 after a verifier refutation. History: the original evidence (commit:c461293f) was true and useless -- that commit really did delete the line, and merge 9c9f27b0 (origin/lane/f13-fix-shared-lib into integ/f13) put it back from the other side of the resolution, so the entry was PRESENT at .config/flaky-allowlist.txt:59 while this criterion read met; `git log -S` does not show it because -S skips merges. The fix was an `absent:` evidence kind that re-reads the file on every gate run. THE VERIFIER THEN BROKE THAT FIX and they were right: the needle was `gh#1182`, the ISSUE TAG, so re-listing the SAME TEST under any other owner (`gh#9999`) left the ledger green -- the guard was one string wide and the string was not the subject of the criterion. The needle is now the test name itself, `contained_construction_does_not_walk_the_workspace`, which is what the criterion is actually about; an entry for this test cannot be resurrected under any tag. RED-ARMED with the verifier's own mutation: re-listing the test under gh#9999 now exits 1 naming this criterion, while the untouched tree exits 0 -- both runs in the same command. The allowlist is 8 dated entries; the known-positive control grep -c crucible_council still returns 1, so the absence is measured by a query that can see a hit."
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
