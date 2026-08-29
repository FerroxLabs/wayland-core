---
issue: 1182
repo: FerroxLabs/wayland
title: "The workspace-walk instrument check declares itself dead under load: a wall-clock ratio decides whether the control is alive"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "Liveness of the workspace walk is established by a direct observation, not by comparing two wall-clock timings"
    state: not-met
    owner: core
    note: "the issue names a visited-entry counter or an injected walker the test can assert was called"
  - id: c2
    text: "The test still goes red when the walk becomes unreachable, which is the property the control exists for"
    state: not-met
    owner: core
    note: "this must survive the rewrite; deleting the control instead of fixing it is the failure mode the issue warns against"
  - id: c3
    text: "The test no longer needs its entry in .config/flaky-allowlist.txt under the #1169 retry-flake gate"
    state: not-met
    owner: core
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
