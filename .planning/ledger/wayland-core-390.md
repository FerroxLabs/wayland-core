---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: fa7a7b168
criteria:
  - id: c1
    text: "A VFS read of an object under a store named by a gitfile on a VENDORED checkout is refused, with that checkout`s own working tree still readable as the wrong-refusal control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_vendored_gitfile_named_store_is_refused_through_the_vfs
    owner: core
    note: "Arm 3 added to is_vcs_content_store: discover_nested_content_stores walks the root, and scan_control_dirs_in reads each nested control directory with EXACTLY the code that reads the root one. A VFS Read of <root>/vendor/pkg-git/objects/12/3456, named by the gitfile <root>/vendor/pkg/.git = `gitdir: ../pkg-git`, is now SecretDenied. Wrong-refusal controls in the same test stay green: the vendored checkout own working tree (vendor/pkg/src/lib.rs), the vendored gitdir HEAD, and an ordinary workspace file. RED ARM, verbatim: nested_stores_memoized body replaced with Vec::new(); cargo check -p wcore-tools --tests exit 0; test fails with `core#390 c1: the point-predicate must call the vendored gitfile store a content store`. Restored, blob identity verified equal to HEAD."
  - id: c2
    text: "The same holds for an objects/info/alternates borrow declared by a NESTED checkout, not only by the workspace root"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_nested_checkouts_alternates_borrow_is_refused_through_the_vfs
    owner: core
    note: "scan_control_dirs_in calls alternate_object_dirs for a NESTED .git, not only the root one. Fixture: <root>/vendor/pkg/.git/objects/info/alternates = `../../../../borrowed/objects`, object at <root>/borrowed/objects/ab/cd1234 — not a lexical (control, store) pair, so arm 1 cannot see it either. Refused; the nested working tree and .git/HEAD stay readable. NAMED GAP, filed rather than hidden: the arm-3 gate is lexical (store_shaped), so a borrow whose TARGET directory is not store-shaped is still admitted. Pinned by a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted and tracked as FerroxLabs/wayland-core#394. Same red arm as c1."
  - id: c3
    text: "Whatever caching the fix introduces is measured against #376`s complaint: the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::an_ordinary_path_never_pays_for_the_nested_store_walk
    owner: core
    note: "STATED AS A NUMBER, two call shapes. (1) ORDINARY GUARD PATH, the shape #376 is about: unchanged at one resolution / zero warm scans / three warm probes, now pinned with arm 3 present, and the cold probe count FELL 17 -> 12. (2) PER-TRAVERSED-DIRECTORY, the shape grep_policy::scope_for calls vcs_content_stores in and which NEITHER the platform lane nor sec-secrets measured: differential strace -f -c over probe_vcs_content_stores_per_traversed_directory at WL_PROBE_DIRS=100 and 1100, hetzner, three arms of the SAME probe with a known-positive control asserting the scan finds <root>/.git/objects in every run — origin/integ/f13 (pre-StoreScan) 8 syscalls/directory; + lane/f13-sec-secrets 17; + this change 5. The middle row is a real 2.1x regression sec-secrets introduced on an unmeasured shape; this change lands BELOW the pre-StoreScan figure. Recorded in the vfs_guard_cost.rs header."
  - id: c4
    text: "When the fix lands, grep_vcs_named_store_deny.rs`s `!is_vcs_content_store` assertion is INVERTED rather than deleted, so the two layers are re-tied"
    state: met
    evidence: test:crates/wcore-tools/tests/grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store
    owner: core
    note: "INVERTED, not deleted. The assertion was `!WorkspacePolicy::contained(&root).is_vcs_content_store(&store_file)` with the note `if the point-predicate has grown a nested arm, this note is stale`; it is now the positive, so Grep and the VFS are re-tied from both sides (vfs_nested_named_store_deny.rs carries the mirror assertion). It went RED on the unmodified test the moment arm 3 landed, which is how the inversion was found rather than chosen."
---

Split out of #244 c1 while closing #244 c3. #244 c1's text was the unqualified
"at any nested depth"; that has been rewritten to the scope that actually holds
and this issue carries the remainder, referenced from #244 c7.

Found by building the second-arm fixture for #244 c3, not reported by that
change's verifier. The Grep half of the same class IS closed in that change --
it is only the in-process VFS predicate that still misses this shape.
