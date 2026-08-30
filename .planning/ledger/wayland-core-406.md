---
issue: 406
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's gate cannot see a store created after the walk at a path it never recorded (residual of #390 c2)"
status: open
last_verified_commit: 972d1c17c
criteria:
  - id: c1
    text: "A nested store created after the arm-3 walk, at a path that is neither store-shaped nor in the last scan's list, is refused on the next guard -- measured with a wrong-refusal control in the same fixture"
    state: not-met
    owner: core
    note: "Filed 2026-08-31 by the w3-vcs-residuals lane while CLOSING core#390 c2, as the residual that close leaves behind -- not a regression, and strictly smaller than what it replaced. WorkspacePolicy::nested_walk_admits decides the arm-3 gate from the last scan's own store list, which makes every store the walk DISCOVERED impossible to hide. What it cannot see is a store that comes into being AFTER that walk at a canonical path the walk never recorded, whose spelling store_shaped also does not recognise: the gate refuses, no revalidation runs, and the memo never gets the chance to notice the mutation. Reachable shapes: a nested control directory appearing mid-session and naming a store at an arbitrary path (a hand-written alternates entry, a .git gitfile); a store leaf symlink re-pointed at a directory the walk has not recorded. A RENAME of a known store is the weakest of the three, because the new path keeps its `objects` component and store_shaped still catches it. BEFORE core#390 c2's fix the ENTIRE class of non-store-shaped stores was admitted whether or not a walk had run, so this is a reduction, not a new hole."
  - id: c2
    text: "The cost of that closure is stated as a number and measured through GuardCounters, and whichever of core#390 c3 / core#398 c2 it moves is RE-GRADED rather than left claiming a figure the tree no longer has"
    state: not-met
    owner: core
    note: "Nothing done. The tension is located, not merely asserted: closing c1 costs at least one filesystem probe on the REFUSE branch, and core#390 c3 pins the ordinary-path guard at exactly three warm probes with the arm-3 contribution at zero. The cheapest sound witness set for the refuse branch is the ANCESTORS of the query path -- every shape in c1 moves the mtime of a directory that is an ancestor of the path being read, and all of those directories are already stamped by the walk -- which is O(depth) probes per guard where the pinned number is zero. `src/deep/deeper/main.rs` would take the ordinary guard from 3 warm probes to 7."
  - id: c3
    text: "store_shaped's remaining role is decided explicitly -- either it is the mutation net and its doc says so, or it is deleted because c1's mechanism subsumes it -- and vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory is re-measured either way"
    state: not-met
    owner: core
    note: "Half done, and the half that is done is recorded in the tree: store_shaped's doc already says it is no longer the arm-3 gate and survives only as the post-scan-mutation net. What is NOT decided is whether it should exist at all. It is also the reason core#398 c1 cannot be closed by simply deleting it: deleting store_shaped flattens that slope to zero AND removes the only thing that catches a store renamed since the walk, so #398 c1 and this criterion have to be answered together. MEASURED on this tree, unchanged by core#390 c2's fix: GATE COST: dirs=8 admitted=19 ordinary=3 | dirs=48 admitted=59 ordinary=3."
---

Split out of FerroxLabs/wayland-core#390 c2 while closing it. Filed by the lane
that wrote the fix, not found later by someone else.

`is_vcs_content_store`'s arm 3 is gated by `WorkspacePolicy::nested_walk_admits`,
which admits a path when the LAST arm-3 scan's own store list already covers it
(plus `store_shaped` as a net, plus a cold-memo arm). That makes every store the
walk DISCOVERED impossible to hide, which is what #390 c2 needed. The residual is
everything that becomes a store AFTER that walk at a path it never recorded.

Criteria are taken verbatim from the issue's Acceptance section.
