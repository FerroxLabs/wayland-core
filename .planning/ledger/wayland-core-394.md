---
issue: 394
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate misses an alternates borrow whose target is not store-shaped (split from #390 c2)"
status: open
last_verified_commit: 972d1c17c
criteria:
  - id: c1
    text: "A VFS Read of an object under an objects/info/alternates borrow declared by a NESTED checkout is refused REGARDLESS of the borrow target's directory name, with that checkout's working tree still readable as the wrong-refusal control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_refused
    owner: core
    note: "CLOSED 2026-08-31 at 972d1c17c. nested_walk_admits decides the arm-3 gate from the scan`s own output rather than from the query path`s spelling, so a borrow target of ANY name is refused. The test named in this criterion is the INVERTED sibling of the one that pinned the gap. Wrong-refusal control in the same fixture, asserted first: an ordinary workspace file stays readable. See core#390 c2 for the red arm (10 tests across 4 files) and the full construction."
  - id: c2
    text: "vfs_guard_cost.rs::an_ordinary_path_never_pays_for_the_nested_store_walk stays green: the ordinary-path guard still costs one resolution, zero extra scans and three warm probes, stated as a number"
    state: not-met
    owner: core
    note: "NOT met, and it MOVED - in the cold direction only, which is why this is graded honestly rather than met with a footnote. The warm number this criterion names is HELD EXACTLY: one resolution, zero warm scans, three warm probes, pinned at two workspace sizes by a_gate_admitted_path_costs_one_probe_per_workspace_directory`s control arm and by one_ordinary_path_guard_resolves_once_and_does_not_rescan. What changed is the cold one-off: the arm-3 walk now runs once per policy on the FIRST guard rather than on the first store-shaped path, because a gate decided by the scan`s output cannot answer before there is one. The first guard therefore scans twice and costs 43 probes where it cost 12 on the vfs_guard_cost fixture. The test this criterion names is renamed an_ordinary_path_pays_for_the_nested_store_walk_at_most_once and grades the steady state as a difference over N guards. Carried alongside core#398 c2."
  - id: c3
    text: "The per-traversed-directory figure grep_policy::scope_for pays is re-measured base-vs-HEAD by the differential-strace probe and does not exceed the 5 syscalls/directory measured at #390's merge"
    state: not-met
    owner: core
    note: "Nothing done. The instrument exists: workspace_policy::tests::probe_vcs_content_stores_per_traversed_directory, #[ignore]d, driven under strace -f -c at WL_PROBE_DIRS=100 and 1100 and differenced. Measured figures at core#390's merge: 8 syscalls/directory before StoreScan, 17 after lane/f13-sec-secrets, 5 after core#390."
  - id: c4
    text: "vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted is INVERTED rather than deleted, so the gap and its closure are graded by the same test"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_refused
    owner: core
    note: "INVERTED, not deleted, exactly as the test`s own assertion message instructed: a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted became a_nested_alternates_borrow_named_nothing_store_like_is_refused, and a wrong-refusal control on an ordinary workspace file was added to the fixture so the refusal cannot be satisfied by a guard that refuses everything. Three more tests were inverted the same way rather than deleted: the partition test in workspace_policy/tests.rs and both root-vs-nested symmetry pairs."
---

Arm 3 of `is_vcs_content_store` (FerroxLabs/wayland-core#390) discovers the content stores that control directories NESTED under the workspace root name, by walking the root. Discovery is the expensive half, so arm 3 is reached only for a path that is `store_shaped` — one carrying a `VCS_CONTENT_STORES` leaf name (`objects`, `modules`, `lfs`, `store`, `pristine`, `repository`) among its components.

That gate is what holds core#376's number: an ordinary path never pays for the walk. A git control directory's own store leaves are FIXED names, so a gitfile-named store cannot escape it. An `objects/info/alternates` borrow can — the entry names an arbitrary directory.

**Scope.** Narrower than it looks: `git clone --shared` / `--reference` and every git porcelain write an object-database path, which is `.../objects`. The admitted shape needs a hand-written `alternates` entry naming something else. It is still a real read of committed content the VFS is supposed to refuse.

**Why it was not closed at #390.** Widening the gate makes every path pay the nested walk — the exact regression #376 was filed about and #390 c3 forbids. The likely right shape is resolving borrow targets eagerly at scan time into a set the point-predicate can test by prefix with no lexical pre-gate; that is a design decision with a measurable cost.
