---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: 875bf32cb
criteria:
  - id: c1
    text: "A VFS read of an object under a store named by a gitfile on a VENDORED checkout is refused, with that checkout`s own working tree still readable as the wrong-refusal control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::a_vendored_gitfile_named_store_is_refused_through_the_vfs
    owner: core
    note: "Arm 3 added to is_vcs_content_store: discover_nested_content_stores walks the root, and scan_control_dirs_in reads each nested control directory with EXACTLY the code that reads the root one. A VFS Read of <root>/vendor/pkg-git/objects/12/3456, named by the gitfile <root>/vendor/pkg/.git = `gitdir: ../pkg-git`, is now SecretDenied. Wrong-refusal controls in the same test stay green: the vendored checkout own working tree (vendor/pkg/src/lib.rs), the vendored gitdir HEAD, and an ordinary workspace file. RED ARM, verbatim: nested_stores_memoized body replaced with Vec::new(); cargo check -p wcore-tools --tests exit 0; test fails with `core#390 c1: the point-predicate must call the vendored gitfile store a content store`. Restored, blob identity verified equal to HEAD."
  - id: c2
    text: "The same holds for an objects/info/alternates borrow declared by a NESTED checkout, not only by the workspace root"
    state: not-met
    evidence: test:crates/wcore-tools/tests/vfs_nested_named_store_deny.rs::the_same_alternates_borrow_is_refused_at_the_root_and_admitted_when_nested
    owner: core
    note: "REGRADED not-met 2026-08-30 after the adversarial verifier's finding 2. The previous pass graded this `met` with the remaining gap named in the note and carried by core#394; that is a substitution, and naming a substitution does not discharge it. c2's axis is root-versus-nested, and the gap is ON that axis, not beside it: MEASURED, not reasoned, by the_same_alternates_borrow_is_refused_at_the_root_and_admitted_when_nested, which drives the SAME borrow target (<root>/odb, not store-shaped, holding the same object) from both declaring control directories with a wrong-refusal control in each arm — declared by <root>/.git it is REFUSED, declared by <root>/vendor/pkg/.git it is ADMITTED. The cause is structural: arm 2's list is consulted with no lexical pre-gate so push_store admits a borrow target of any name, while arm 3's is reachable only through store_shaped. So `the same holds` is false as written. WHAT DID LAND, and is real: scan_control_dirs_in calls alternate_object_dirs for a NESTED .git, not only the root one. Fixture: <root>/vendor/pkg/.git/objects/info/alternates = `../../../../borrowed/objects`, object at <root>/borrowed/objects/ab/cd1234 — not a lexical (control, store) pair, so arm 1 cannot see it either. Refused; the nested working tree and .git/HEAD stay readable. NAMED GAP, filed rather than hidden: the arm-3 gate is lexical (store_shaped), so a borrow whose TARGET directory is not store-shaped is still admitted. Pinned by a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted and its new root-declared sibling, and carried by FerroxLabs/wayland-core#394 (OPEN, filed 2026-08-30T08:53:20Z, cited by number from store_shaped's doc). Same red arm as c1: nested_stores_memoized -> Vec::new() reddens the store-shaped arm and leaves the root arm green, which is what proves the two arms are answered by different code."
  - id: c3
    text: "Whatever caching the fix introduces is measured against #376`s complaint: the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number"
    state: not-met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory
    owner: core
    note: "REGRADED not-met 2026-08-30. c3's text is unqualified — `the per-operation cost of is_vcs_content_store does not get worse than it is today` — and the previous pass graded it against the two shapes it had measured while leaving a THIRD unmeasured. That third shape is the one arm 3's own gate creates, and it is worse. MEASURED by a_gate_admitted_path_costs_one_probe_per_workspace_directory (counted through GuardCounters, not timed): a warm guard on <root>/modules/vpc/main.tf — an ordinary Terraform layout with no control directory, gitfile or store anywhere under it — costs 19 probes in an 8-directory workspace and 59 in a 48-directory one, a slope of exactly 1.000 filesystem probe per workspace directory, where the gate-refused path stays at 3 in both as the control. A/B: the SAME test file at 1b9cb34d5 (the tree #390 landed on) fails with `left: 0, right: 40` and prints admitted=3 at both sizes, so the slope is arm 3's and not the fixture's. store_shaped opens on `objects`, `modules`, `lfs`, `store`, `pristine`, `repository`, which are ordinary project directory names, so this is reachable by ordinary reads. Carried by FerroxLabs/wayland-core#398. WHAT WAS MEASURED AND STANDS, restated in ONE unit (TOTAL syscalls per traversed directory, differenced over WL_PROBE_DIRS 100/1100/2100, two repetitions, `1 passed` asserted in all runs): 8 at 4c55f5ac6 (7 statx + 1 openat), 17 at 1b9cb34d5 (16 statx + 1 openat), 5 at 875bf32cb (5 statx). The 16-vs-17 disagreement between this file and scan_control_dirs_in's comment was a UNIT mismatch, statx-only against total, not a measurement error; both now quote the total. Two call shapes as before: (1) ORDINARY GUARD PATH, the shape #376 is about: unchanged at one resolution / zero warm scans / three warm probes, now pinned with arm 3 present, and the cold probe count FELL 17 -> 12. (2) PER-TRAVERSED-DIRECTORY, the shape grep_policy::scope_for calls vcs_content_stores in and which NEITHER the platform lane nor sec-secrets measured: differential strace -f -c over probe_vcs_content_stores_per_traversed_directory at WL_PROBE_DIRS=100 and 1100, hetzner, three arms of the SAME probe with a known-positive control asserting the scan finds <root>/.git/objects in every run — origin/integ/f13 (pre-StoreScan) 8 syscalls/directory; + lane/f13-sec-secrets 17; + this change 5. The middle row is a real 2.1x regression sec-secrets introduced on an unmeasured shape; this change lands BELOW the pre-StoreScan figure. Recorded in the vfs_guard_cost.rs header."
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
