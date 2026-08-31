---
issue: 394
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate misses an alternates borrow whose target is not store-shaped (split from #390 c2)"
status: open
last_verified_commit: 30fd6cfde
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
    owner: core
    note: "The named instrument does not exist in this lineage: `vfs_guard_cost.rs::an_ordinary_path_never_pays_for_the_nested_store_walk` is on `lane/f13-platform`/`lane/f13-w3-vcs-residuals`, neither of which is an ancestor of `integ/f13`. The PROPERTY is measured and holds, by the test this lineage does have: `one_ordinary_path_guard_resolves_once_and_does_not_rescan` -- 1 resolution per guard, scans stays at 1 over 50 further guards, and warm probes are exactly N*3. Arm 3 contributes ZERO probes on an ordinary path (no ancestor carries a store leaf name) and arm 4 contributes zero once warm. Graded not-met-as-written rather than met, because the criterion names a test and this lane cannot keep a test green that is not here."
  - id: c3
    text: "The per-traversed-directory figure `grep_policy::scope_for` pays is re-measured base-vs-HEAD by the differential-strace probe (`probe_vcs_content_stores_per_traversed_directory`) and does not exceed the 5 syscalls/directory measured at #390's merge."
    state: not-met
    owner: core
    note: "Unreproducible as written on this lineage: the '5 syscalls/directory measured at #390's merge' was measured on `lane/f13-w3-vcs-residuals`, which is NOT an ancestor of `integ/f13`, and the probe it used is not in the tree. The comparable measurement was made instead, base-vs-HEAD on the same fixture with a probe added here: 35.009 -> 34.997 total syscalls per traversed directory, known-positive control (`scope_for` must still classify the root store) asserted green in every run. No regression; the absolute figure is not comparable to 5.000 because the fixtures differ (this one writes a file in each directory)."
  - id: c4
    text: "`vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted` is INVERTED rather than deleted, so the gap and its closure are graded by the same test."
    state: not-met
    owner: core
    note: "`vfs_nested_named_store_deny.rs` does not exist in this lineage, so its assertion cannot be inverted rather than deleted. The property it would grade is asserted directly by `vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named`. Recorded as not-met-as-written rather than quietly re-pointed at a different file."
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
