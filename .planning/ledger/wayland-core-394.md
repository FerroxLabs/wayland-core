---
issue: 394
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate misses an alternates borrow whose target is not store-shaped (split from #390 c2)"
status: open
last_verified_commit: fa7a7b168
criteria:
  - id: c1
    text: "A VFS Read of an object under an objects/info/alternates borrow declared by a NESTED checkout is refused REGARDLESS of the borrow target's directory name, with that checkout's working tree still readable as the wrong-refusal control"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by the w3-vcs-residuals lane while closing core#390 c2, as a NAMED GAP in the gate that fix had to introduce -- not a regression. Measured, not modelled: crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted asserts fs.read(&borrowed).await.is_ok() for <root>/vendor/pkg/.git/objects/info/alternates = `../../../../odb` with the object at <root>/odb/ab/cd1234, and it PASSES. The sibling test one function up, identical except that the borrow target is named <root>/borrowed/objects, is refused."
  - id: c2
    text: "vfs_guard_cost.rs::an_ordinary_path_never_pays_for_the_nested_store_walk stays green: the ordinary-path guard still costs one resolution, zero extra scans and three warm probes, stated as a number"
    state: not-met
    owner: core
    note: "The cost side of the same fix. Nothing done; the number to hold is the one core#390 c3 recorded -- one resolution / zero warm scans / three warm probes on the ordinary guard path, 12 cold probes."
  - id: c3
    text: "The per-traversed-directory figure grep_policy::scope_for pays is re-measured base-vs-HEAD by the differential-strace probe and does not exceed the 5 syscalls/directory measured at #390's merge"
    state: not-met
    owner: core
    note: "Nothing done. The instrument exists: workspace_policy::tests::probe_vcs_content_stores_per_traversed_directory, #[ignore]d, driven under strace -f -c at WL_PROBE_DIRS=100 and 1100 and differenced. Measured figures at core#390's merge: 8 syscalls/directory before StoreScan, 17 after lane/f13-sec-secrets, 5 after core#390."
  - id: c4
    text: "vfs_nested_named_store_deny.rs::a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted is INVERTED rather than deleted, so the gap and its closure are graded by the same test"
    state: not-met
    owner: core
    note: "Nothing done. The same discipline core#390 c4 applied to grep_vcs_named_store_deny.rs, and the test's own assertion message says so."
---

Arm 3 of `is_vcs_content_store` (FerroxLabs/wayland-core#390) discovers the content stores that control directories NESTED under the workspace root name, by walking the root. Discovery is the expensive half, so arm 3 is reached only for a path that is `store_shaped` — one carrying a `VCS_CONTENT_STORES` leaf name (`objects`, `modules`, `lfs`, `store`, `pristine`, `repository`) among its components.

That gate is what holds core#376's number: an ordinary path never pays for the walk. A git control directory's own store leaves are FIXED names, so a gitfile-named store cannot escape it. An `objects/info/alternates` borrow can — the entry names an arbitrary directory.

**Scope.** Narrower than it looks: `git clone --shared` / `--reference` and every git porcelain write an object-database path, which is `.../objects`. The admitted shape needs a hand-written `alternates` entry naming something else. It is still a real read of committed content the VFS is supposed to refuse.

**Why it was not closed at #390.** Widening the gate makes every path pay the nested walk — the exact regression #376 was filed about and #390 c3 forbids. The likely right shape is resolving borrow targets eagerly at scan time into a set the point-predicate can test by prefix with no lexical pre-gate; that is a design decision with a measurable cost.
