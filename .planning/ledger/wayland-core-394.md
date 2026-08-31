---
issue: 394
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate misses an alternates borrow whose target is not store-shaped (split from #390 c2)"
status: open
last_verified_commit: 967bdf2fb
criteria:
  - id: c1
    text: "A VFS `Read` of an object under an `objects/info/alternates` borrow declared by a NESTED checkout is refused REGARDLESS of the borrow target's directory name, with that checkout's working tree still readable as the wrong-refusal control."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named
    owner: core
    note: "MEASURED. `vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named` -- borrow target `<root>/odb`, no store component in the path, refused through the production stack with the declaring checkout's working tree readable as the control. The second route the ticket's own comment reported (a `.git/objects` SYMLINK to a non-store-shaped directory) is graded separately by `a_symlinked_nested_store_leaf_is_refused_by_its_resolved_path`, BY THE RESOLVED PATH, which is the harder arm. Closed the way this ticket's body proposed -- borrow targets resolved eagerly at scan time into a set tested by prefix -- and NOT by widening a lexical gate: there is no lexical pre-gate in this lineage to widen, because arm 4 costs zero filesystem probes once warm."
  - id: c2
    text: "`vfs_guard_cost.rs::an_ordinary_path_never_pays_for_the_nested_store_walk` stays green: the ordinary-path guard still costs one resolution, zero extra scans and three warm probes. Stated as a number."
    state: not-met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan
    owner: core
    note: NOT MET AS WRITTEN: the test the criterion names does not exist in this lineage. `grep -rl an_ordinary_path_never_pays_for_the_nested_store_walk --include=*.rs crates/` returns 0 hits, with a known-positive control in the same session (`one_ordinary_path_guard_resolves_once_and_does_not_rescan` returns 1) so the zero is a real absence and not a broken query. A criterion that says `THIS test stays green` cannot be met by a test that was never in the tree.\nTHE PROPERTY IS MET AND MEASURED, by the test that does the same job here. `vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan`, green at HEAD: exactly ONE resolution per guard over 51 guards, `scans` stays at 1 (zero extra scans), warm probes exactly N*3 - one resolution / zero extra scans / three warm probes, the three numbers the criterion asks for, UNCHANGED from base. MEASURED base-vs-HEAD on hetzner, base `ca15a48bf` vs HEAD `967bdf2fb` (lane/f13-s2-vcs-cost). Two DISTINCT binaries, identity proven by content and not by date: sha256 differs and `strings base_bin | grep -c nested_declarations_moved` = 0 against 5 for HEAD.\nThis lane`s change is the reason the number could have moved and did not: arm 4`s freshness check revalidates the DECLARATION SITES the walk read, and a workspace with no nested checkout has none, so the witness set is empty and the check costs zero probes. Pinned as a number by `the_post_walk_freshness_check_scales_with_checkouts_not_directories`, which asserts (3, 3) for the no-checkout arms at both workspace sizes before it measures anything else.\nPROPOSED REPOINT, on the ISSUE: name `vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan`.
  - id: c3
    text: "The per-traversed-directory figure `grep_policy::scope_for` pays is re-measured base-vs-HEAD by the differential-strace probe (`probe_vcs_content_stores_per_traversed_directory`) and does not exceed the 5 syscalls/directory measured at #390's merge."
    state: not-met
    owner: core
    note: MEASURED base-vs-HEAD on hetzner, base `ca15a48bf` vs HEAD `967bdf2fb` (lane/f13-s2-vcs-cost). Two DISTINCT binaries, identity proven by content and not by date: sha256 differs and `strings base_bin | grep -c nested_declarations_moved` = 0 against 5 for HEAD.\nDifferential `strace -f -c` over `workspace_policy::tests::probe_vcs_content_stores_per_traversed_directory`, SAME fixture both arms (a root `.git` plus WL_PROBE_DIRS ordinary `pkg{i}/main.rs` directories), arms INTERLEAVED at every configuration so host-load drift hits both, THREE operation counts (WL_PROBE_DIRS 100/1100/2100) x WL_PROBE_REPS 1/6. The probe`s own known-positive control (`1 passed` and `scope_for` still classifying the root store) asserted green in all 12 runs.\nDifferencing REPS 6 against REPS 1 at the same directory count cancels arm 4`s one-off walk, which is itself O(directories) and is otherwise indistinguishable from a per-traversal cost:\n  base 100->1100  steady 34.998 syscalls/traversed dir   (one-off 18.988)\n  base 1100->2100 steady 35.000                          (one-off 19.084)\n  HEAD 100->1100  steady 35.001                          (one-off 19.039)\n  HEAD 1100->2100 steady 35.001                          (one-off 19.012)\nSo this change moves the figure by <= 0.003 syscalls/directory, which is below the run-to-run spread of the instrument itself (a second full interleaved pass earlier in the same session read 35.001 on BOTH arms).\nTHE 5 SYSCALLS/DIRECTORY FIGURE IS NOT REPRODUCIBLE ON THIS LINEAGE AND THE CRITERION CANNOT BE MET AS WRITTEN. It was measured at `875bf32cb` on `lane/f13-w3-vcs-residuals`; `git merge-base --is-ancestor 875bf32cb HEAD` is FALSE, as it is for `972d1c17c`. That lineage`s arm 3 gated a whole-tree walk on a path spelling; this one (`0ed5d4707`, an ancestor - verified YES) resolves stores eagerly into a `OnceLock`-style set and its `grep_policy::scope_for` asks `denies_read_content`. Different traversal, different fixture, different tree: the two numbers are not comparable, and 5.000 is not a bar this code can be held to. TWO INDEPENDENT LANES HAVE NOW MEASURED ~35 HERE (lane/f13-vcs-store read 35.009 -> 34.997; this lane reads 34.998-35.001 across three operation counts and two passes), which is the reproducibility the 5.000 lacks.\nPROPOSED REPOINT, to be made ON THE ISSUE and not here: replace `does not exceed the 5 syscalls/directory measured at #390`s merge` with `does not exceed 35.1 syscalls per traversed directory on the probe_vcs_content_stores_per_traversed_directory fixture (a root .git plus WL_PROBE_DIRS ordinary directories), re-measured base-vs-HEAD, interleaved, with the probe`s known-positive control green in every run`. The fixture is named because the previous figure`s unreproducibility is exactly a fixture difference.
  - id: c4
    text: "`vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted` is INVERTED rather than deleted, so the gap and its closure are graded by the same test."
    state: not-met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused
    owner: core
    note: NOT MET AS WRITTEN: neither the file nor the test the criterion names exists in this lineage. `grep -rl vfs_nested_named_store_deny --include=*.rs crates/` -> 0 hits and `grep -rl a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted --include=*.rs crates/` -> 0 hits, both with the known-positive control above green in the same session. The file that carries these shapes here is `vfs_nested_store_deny.rs` (no `named`), on the `integ/f13` lineage; the `_named_` spelling belongs to `lane/f13-w3-vcs-residuals` @ 972d1c17c, which is NOT an ancestor of HEAD.\nTHE INVERT-DO-NOT-DELETE DISCIPLINE THE CRITERION IS ABOUT WAS HONOURED LITERALLY IN THIS LANE, on the test that plays the same role: `vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_still_admitted` asserted the leak and its failure message instructed its own inversion. It is now `..._is_refused`, same file, same fixture, same construction, with the wrong-refusal control kept on BOTH sides of the mutation and `nested_walk_count() == 1` asserted first so the test grades the post-walk state and not a cold policy. Nothing was deleted.\nPROPOSED REPOINT, on the ISSUE: name `vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused` as the inverted form.
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
