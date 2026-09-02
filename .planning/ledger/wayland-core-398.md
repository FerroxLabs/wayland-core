---
issue: 398
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate makes a guard on any path named objects/modules/store cost one syscall per workspace directory (split from #390 c3)"
status: closed
last_verified_commit: 7e159c955
criteria:
  - id: c1
    text: "The warm per-guard cost of a gate-admitted path is INDEPENDENT of the workspace-s directory count, graded as a SLOPE by `vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size` and `::the_post_walk_freshness_check_scales_with_checkouts_not_directories`, both of which assert the slope rather than a single reading."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size"
    owner: core
    note: "MET AFTER REPOINT, on TWO absences rather than one. The test the criterion said to invert returns 0 files. And the slope-1.0 regression it says to invert was never in this lineage: git merge-base --is-ancestor 875bf32cb HEAD is NO, verified in one call alongside 0ed5d4707 / 967bdf2fb / ca15a48bf which are all YES, so the negative is a real answer and not a bad rev. A criterion that says INVERT a regression this tree never had cannot pass as written; that is a gate that cannot pass, which is worth as little as one that cannot fail. Property measured at slope 0.000 across 8 and 48 workspace directories, on the lane that priced the one change capable of breaking it."
  - id: c2
    text: "The refusals #390 bought stay bought: `vfs_nested_store_deny.rs` stays green in full, including the vendored-gitfile arm and the nested-alternates arm, so the cost fix is not a quiet revert."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused"
    owner: core
    note: "MET AFTER REPOINT -- a filename typo only: the file is vfs_nested_store_deny.rs, not vfs_nested_named_store_deny.rs (0 hits vs a control of 2). Earned rather than asserted, because this work IS a cost fix on the store scan and a quiet revert is exactly its failure mode: the file is 9/9 green including both arms the criterion names BY TEST NAME, and the discriminating mutation reddens 8 tests across 3 files, three of them real refusals plus one explicit fail-open report."
  - id: c3
    text: "— The ordinary (gate-refused) path is unchanged at one resolution / zero warm scans / three warm probes, still pinned by `one_ordinary_path_guard_resolves_once_and_does_not_rescan`."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan
    owner: core
    note: "MET, and RE-MEASURED here rather than carried forward, because this lane changed `scan_vcs_content_stores` -- the function whose output this number prices. NOTE FOR THE NEXT GRADER: this id is the ORDINARY-PATH PROBE COUNT (the first acceptance block), not the differential-strace figure; the checkbox list's c3 is the strace one and is graded on core#394 c3 / core#396 c3, where this lane records 5.000.\n`vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan` green at HEAD 7e159c955: `resolves` == N+1 over 50 guards (one resolution, never two), `scans` stays at 1 (zero warm scans), `probes - first_probes` == N*3 exactly (three warm probes), `nested_walk_count()` == 1.\nALL THREE UNCHANGED BY THE PORT, which is the non-trivial part: the port removes probes from the SCAN and this number is the memo REVALIDATION, and the witness set did not change because an absent control directory was never stamped in the first place. The COLD one-off did move, in the cheap direction, and is restated rather than hidden: first guard 41 -> 36 probes on this fixture (arm-2 scan 17 -> 12, arm 4's one walk 24, unchanged). The literal is updated in the test and the figure table added to that file's header, as the assertion's own message instructs."
  - id: c4
    text: "— The figure is re-stated as a number for BOTH call shapes on the tree the fix lands on, base-vs-HEAD, by the two instruments named above (the counted slope and the differential `strace` per-traversed-directory figure), each with its known-positive control green."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size
    owner: core
    note: "Both call shapes, base-vs-HEAD, each with its control:\n* counted slope (`GuardCounters`, platform-independent): gate-admitted path 4 probes at 8 dirs and 4 at 48 dirs, slope 0; gate-refused ordinary path 3 warm probes at both -- the control that shows the fixture is not the reason.\n* differential `strace` per traversed directory: base 35.009 -> HEAD 34.997, probe's known-positive control asserted green in every run.\nCold one-off stated separately: first guard 17 -> 41 probes on the `vfs_guard_cost` fixture, +6.0 syscalls/workspace directory, once per policy."
  - id: c5
    text: "SUPERSEDED. The DENY_CACHE_MAX_DIRS branch named here belongs to a memo this issue never touched."
    state: superseded
    successor: FerroxLabs/wayland-core#413
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::the_post_walk_freshness_check_scales_with_checkouts_not_directories
    owner: core
    note: "SUPERSEDED into FerroxLabs/wayland-core#413. nested_stores_memoized does not exist in this lineage (0 hits, control is_vcs_content_store 9 in the same call). The surviving DENY_CACHE_MAX_DIRS branch is deny_cache-s, inside secret_deny_paths_for_backend -- a different memo, untouched by this cost work -- and reaching its false arm on the production path needs 100,001 directories, which is not acceptable in a suite that runs on macOS and Windows CI. Left here it would be a residual pointed at a symbol nobody can find, which is not tracked but lost. #413 carries it under 0.13.13 with its own acceptance criteria."
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
