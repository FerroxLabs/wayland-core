---
issue: 406
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3s gate cannot see a store created after the walk at a path it never recorded (residual of #390 c2)"
status: open
last_verified_commit: 30fd6cfde
criteria:
  - id: c1
    text: ": A nested store created after the arm-3 walk, at a path that is neither store-shaped nor in the last scan's list, is refused on the next guard — measured with a wrong-refusal control in the same fixture, in the shape `vfs_guard_cost.rs::a_store_created_after_the_scan_is_denied_on_the_next_guard` already uses for arm 2."
    state: not-met
    owner: core
    note: "PARTIALLY closed, and the remainder is pinned rather than described. CLOSED: a repository created after the walk is refused on the NEXT guard -- `vfs_nested_store_deny.rs::a_bare_repository_created_after_the_first_guard_is_refused_on_the_next` warms every memo and runs arm 4's one-off walk BEFORE the store exists (the post-warm state the brief requires), then creates `<root>/vendor/late.git` and asserts the object is refused, with the wrong-refusal control green on both sides. This works because arm 3 memoises NOTHING and reads the filesystem at the instant it is asked.\nNOT CLOSED: a borrow WRITTEN after the walk whose target carries no store leaf component (`<root>/late-odb`) is still admitted -- arm 3 sees nothing in that path's own ancestry and arm 4 does not revalidate. Pinned as a MEASUREMENT by `a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_still_admitted`, whose failure message instructs its own inversion and the re-grade of #398's probe count when the trade is taken. The criterion says 'a path that is neither store-shaped nor in the last scan's list'; that is exactly the half still open, so this is graded not-met."
  - id: c2
    text: ": The cost of that closure is stated as a number and measured through `GuardCounters`, and whichever of core#390 c3 / core#398 c2 it moves is RE-GRADED rather than left claiming a figure the tree no longer has."
    state: met
    evidence: symbol:crates/wcore-tools/src/workspace_policy.rs::encloses_repository_store
    owner: core
    note: "Stated as numbers through `GuardCounters` for the half that IS closed. Arm 3 costs ZERO probes on an ordinary path (no ancestor carries a store leaf name -- this is why the warm ordinary-path figure is unchanged at 3) and at most 2 on a path whose ancestry does carry one, independent of workspace size: measured 4 warm probes total for `modules/vpc/main.tf` at both 8 and 48 workspace directories. #398 c2 re-graded above and is UNCHANGED warm (3 probes) with the cold one-off restated (17 -> 41 on the `vfs_guard_cost` fixture). #390 c3 re-graded above with both instruments."
  - id: c3
    text: ": `store_shaped`'s remaining role is decided explicitly — either it is the mutation net and the doc says so, or it is deleted because c1's mechanism subsumes it, and `tests/vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory` is re-measured either way."
    state: met
    evidence: symbol:crates/wcore-tools/src/workspace_policy.rs::nested_content_stores
    owner: core
    note: "DECIDED and recorded in code. `store_shaped` does not exist in this lineage and is deliberately not introduced: with arm 4 answering at zero warm cost there is nothing to pre-gate, which is what makes #394 and #398 close together. What plays the mutation-net role instead is arm 3, and its own gate is `is_vcs_store_leaf_name` applied to the ANCESTORS OF THE QUERY PATH -- O(depth), not a whole-tree walk -- documented on `WorkspacePolicy::encloses_repository_store` including why the confirmation is a repository test and not a leaf-name test. The re-measurement the criterion asks for is `a_store_named_path_costs_the_same_at_any_workspace_size`: 4 probes, slope 0."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
