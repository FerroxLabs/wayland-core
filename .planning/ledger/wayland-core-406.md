---
issue: 406
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3s gate cannot see a store created after the walk at a path it never recorded (residual of #390 c2)"
status: closed
last_verified_commit: 7e159c955
criteria:
  - id: c1
    text: "A nested store created after the arm-3 walk by a control directory the walk FOUND, at a path that is neither store-shaped nor in the last scan-s list, is refused on the next guard -- measured with a wrong-refusal control in the same fixture."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused"
    owner: core
    note: "MET AFTER REPOINT, and the half that was cut out is a DECISION recorded rather than work dropped. Closed here: a borrow written after the walk by a control directory the walk found is refused, wrong-refusal control green on both sides, nested_walk_count()==1 asserted before the mutation so the test grades the POST-WALK state. Cut out: a control directory created after the walk. That half was not modelled, it was RUN -- witnessing descended directories in discover_nested_content_stores costs the warm gate-admitted guard 15 probes at 8 directories and 95 at 48, slope 2.000 per directory, cold 36 -> 44. On a 10,000-directory checkout that is ~20,000 symlink_metadata calls per file read, on a SecretDenyFs installed for every sub-agent, and it is #398 c1-s regression at twice the rate #398 itself reported -- against #398 c1, #398 c2, #394 c2 and #390 c3, which all pin the warm ordinary guard at three probes. Four criteria and the product-s read latency against one shape that needs a checkout to appear mid-session AND declare an alternates borrow AND target a path carrying no store leaf. Not taken. No bounded witness set can see it either -- the borrow target is not an ancestor of the query path -- so the price is inherent, and the residual is pinned live by a_borrow_declared_by_a_control_dir_created_after_the_walk_is_still_admitted, whose failure message instructs its own inversion."
  - id: c2
    text: ": The cost of that closure is stated as a number and measured through `GuardCounters`, and whichever of core#390 c3 / core#398 c2 it moves is RE-GRADED rather than left claiming a figure the tree no longer has."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::the_post_walk_freshness_check_scales_with_checkouts_not_directories
    owner: core
    note: "MET, and re-verified at HEAD 7e159c955 with the cost now stated for BOTH halves of c1 rather than only the closed one.\nTHE CLOSED HALF, counted through `GuardCounters` by `the_post_walk_freshness_check_scales_with_checkouts_not_directories` (green): warm probes per admitted guard = 3 with no nested checkout, 5 with one vendored checkout, 7 with two -- and IDENTICAL at 8 and at 48 workspace directories. The price is O(nested checkouts) and flat in the tree, which is why core#398 c1's slope stayed at 0.000 while this closed. Arm 3 costs zero probes on an ordinary path and at most two on a path whose ancestry carries a store leaf name (4 warm probes total, slope 0, by `a_store_named_path_costs_the_same_at_any_workspace_size`).\nTHE OPEN HALF IS NOW PRICED TOO, by running that test's OWN NAMED RED ARM rather than reasoning about it: witnessing the descended directories closes the leak and costs slope 2.000 probes per workspace directory, taking the no-checkout warm guard from (3, 3) to (15, 95). Full detail under c1.\nAND THE RE-GRADE THIS CRITERION DEMANDS OF WHOEVER MOVES A FIGURE HAS BEEN DONE. This lane moved two: `vcs_content_stores` 17.000 -> 5.000 syscalls per traversed directory and the cold first guard 41 -> 36 probes. core#390 c3, core#394 c2/c3, core#396 c3 and core#398 c1/c2/c3 are each re-graded on their own tickets against the tree that now exists, and the `vfs_guard_cost.rs` header carries the new figure table so the next reader is not comparing against a number the tree no longer has."
  - id: c3
    text: ": `store_shaped`'s remaining role is decided explicitly — either it is the mutation net and the doc says so, or it is deleted because c1's mechanism subsumes it, and `tests/vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory` is re-measured either way."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size
    owner: core
    note: "MET, and re-decided rather than carried forward, because this lane changed what the answer rests on.\nDECIDED: `store_shaped` does not exist in this lineage, is deliberately not introduced, and - the part that is new here - IS NOT NEEDED EVEN NOW THAT A FRESHNESS CHECK EXISTS. The previous decision was recorded against a tree in which nothing had yet had to pay for freshness, so it was untested. A lexical pre-gate exists to stop an ordinary path paying for a whole-tree walk; the check core#406 c1 required costs an ordinary path ZERO probes (empty witness set) and a checkout-bearing workspace 2 per checkout, so there is still nothing to gate. Adding one would buy nothing and would reintroduce exactly the blindness core#394 records.\nWHAT PLAYS THE MUTATION-NET ROLE, documented in code and not only here: arm 3 (`WorkspacePolicy::encloses_repository_store`) for anything repository-shaped - `is_vcs_store_leaf_name` applied to the ANCESTORS OF THE QUERY PATH, O(depth) not O(workspace), memoising nothing so it is fresh at the instant it is asked - and, added by this lane, arm 4's declaration-site revalidation for a borrow whose target has neither a store name nor a repository shape. Both are on `is_vcs_content_store_resolved` / `nested_declarations_moved` with the reasoning at the site.\nRE-MEASURED EITHER WAY, as the criterion requires. The named test (`a_gate_admitted_path_costs_one_probe_per_workspace_directory`) is not in this lineage - 0 grep hits with a known-positive control green - and core#398 c1 records that pointer defect and the proposed repoint. The measurement itself was performed, base-vs-HEAD, on the tests that do that job here: `a_store_named_path_costs_the_same_at_any_workspace_size` reads 4 warm probes at 8 AND 48 workspace directories, slope 0.000, unchanged from base; `the_post_walk_freshness_check_scales_with_checkouts_not_directories` reads slope 0.000 for the checkout-bearing arms too. The re-measurement is what makes this a decision rather than a claim: the shape that would have forced `store_shaped` back is a per-directory cost, and there is none."
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
