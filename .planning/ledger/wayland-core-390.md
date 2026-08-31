---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: 967bdf2fb
criteria:
  - id: c1
    text: "A VFS read of an object under a store named by a gitfile on a VENDORED checkout is refused, with that checkout`s own working tree still readable as the wrong-refusal control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_vendored_gitfiles_object_store_is_refused
    owner: core
    note: "MEASURED at lane/f13-vcs-store. `vfs_nested_store_deny.rs::a_vendored_gitfiles_object_store_is_refused`: `<root>/vendor/pkg/.git` = `gitdir: ../pkg-git`, object at `<root>/vendor/pkg-git/objects/12/3456`, driven through the production `SandboxedFs(SecretDenyFs(RealFs, contained(root)))` stack -- refused, with TWO controls green (an ordinary source file, and that checkout's own working tree `vendor/pkg/README.md`). Closed by arm 4 (`discover_nested_content_stores`), which reads a gitfile where it LIES instead of only at `<root>/.git`. RED ARM (compiled first, check clean): remove arms 3+4 and 6 of the 8 tests in that file go red with the canary in the bytes; the negative control and the recorded residual stay green."
  - id: c2
    text: "The same holds for an objects/info/alternates borrow declared by a NESTED checkout, not only by the workspace root"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named
    owner: core
    note: "`a_nested_alternates_borrow_is_refused_whatever_its_target_is_named`: `<root>/vendor/pkg/.git/objects/info/alternates` = `../../../../odb`, object at `<root>/odb/ab/cd1234` -- a target carrying NO store component. Refused. Arm 4 resolves borrow targets eagerly at scan time and the set is tested by prefix, so the target's directory name never enters the decision; that is also what closes #394 c1 with the same mechanism. Declaring checkout's working tree readable as the control."
  - id: c3
    text: "Whatever caching the fix introduces is measured against #376`s complaint: the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number"
    state: not-met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::the_post_walk_freshness_check_scales_with_checkouts_not_directories
    owner: core
    note: RE-GRADED 2026-08-31 (lane/f13-s2-vcs-cost) because THIS LANE MOVED THE NUMBER, which is what core#406 c2 requires of whoever moves it. Not-met, and the reason is a trade taken deliberately rather than a regression found later.\nMEASURED base-vs-HEAD on hetzner, base `ca15a48bf` vs HEAD `967bdf2fb` (lane/f13-s2-vcs-cost). Two DISTINCT binaries, identity proven by content and not by date: sha256 differs and `strings base_bin | grep -c nested_declarations_moved` = 0 against 5 for HEAD.\nCounted through `GuardCounters` (platform-independent; a wall clock cannot tell a skipped scan from a fast one), warm per-guard filesystem probes:\n                                                  base   HEAD\n  ordinary path, no nested checkout                  3      3   <- UNCHANGED\n  store-leaf-named path (`modules/vpc/main.tf`)      4      4   <- UNCHANGED\n  ordinary path, ONE vendored checkout               3      5   <- +2\n  ordinary path, TWO vendored checkouts              3      7   <- +2 each\n  slope across 8 vs 48 workspace directories         0      0   <- UNCHANGED\nThe base column is measured, not assumed: it is the same test run against a mutation that neuters `nested_declarations_moved` to `return false`, which is precisely base behaviour (the memo trusted forever). That mutation COMPILES (`cargo check -p wcore-tools --tests` RC=0) so the reds are behaviour, not a build break, and it reddens exactly two tests while every pre-existing refusal and every wrong-refusal control stays green.\nSO: for a workspace with no nested checkout - the common case, and the one core#376 measured - the per-operation cost of `is_vcs_content_store` is byte-for-byte what it was. For a workspace with N vendored checkouts an ADMITTED guard now costs 2N more probes, because closing core#406 c1 means the branch that is about to admit must ask whether the declarations arm 4 read have changed, and core#406`s own body already located the tension there: any per-call freshness check for a whole-tree fact costs at least one probe. The cost is O(nested checkouts) and NOT O(directories), which is the property that let core#398 c1`s slope stay at zero while this closed.\nDifferential `strace -f -c` over `workspace_policy::tests::probe_vcs_content_stores_per_traversed_directory`, SAME fixture both arms (a root `.git` plus WL_PROBE_DIRS ordinary `pkg{i}/main.rs` directories), arms INTERLEAVED at every configuration so host-load drift hits both, THREE operation counts (WL_PROBE_DIRS 100/1100/2100) x WL_PROBE_REPS 1/6. The probe`s own known-positive control (`1 passed` and `scope_for` still classifying the root store) asserted green in all 12 runs.\nDifferencing REPS 6 against REPS 1 at the same directory count cancels arm 4`s one-off walk, which is itself O(directories) and is otherwise indistinguishable from a per-traversal cost:\n  base 100->1100  steady 34.998 syscalls/traversed dir   (one-off 18.988)\n  base 1100->2100 steady 35.000                          (one-off 19.084)\n  HEAD 100->1100  steady 35.001                          (one-off 19.039)\n  HEAD 1100->2100 steady 35.001                          (one-off 19.012)\nSo this change moves the figure by <= 0.003 syscalls/directory, which is below the run-to-run spread of the instrument itself (a second full interleaved pass earlier in the same session read 35.001 on BOTH arms).\nGraded not-met rather than met because the criterion says `does not get worse than it is today` without qualification, and for a workspace with a vendored checkout it did get worse. Whether 2 probes per checkout is the right price for the refusal it buys is a call for the issue, not for this note - it is stated as a number here so it can be made.
  - id: c4
    text: "When the fix lands, grep_vcs_named_store_deny.rs`s `!is_vcs_content_store` assertion is INVERTED rather than deleted, so the two layers are re-tied"
    state: met
    evidence: test:crates/wcore-tools/tests/grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store
    owner: core
    note: "INVERTED, not deleted. `grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store` carried `assert!(!policy.is_vcs_content_store(&store_file))` as the record that Grep and the point-predicate disagreed; it now asserts the predicate DOES see the vendored gitfile's store, with a comment saying which ticket inverted it and why. 3 passed."
---

Split out of #244 c1 while closing #244 c3. #244 c1's text was the unqualified
"at any nested depth"; that has been rewritten to the scope that actually holds
and this issue carries the remainder, referenced from #244 c7.

Found by building the second-arm fixture for #244 c3, not reported by that
change's verifier. The Grep half of the same class IS closed in that change --
it is only the in-process VFS predicate that still misses this shape.
