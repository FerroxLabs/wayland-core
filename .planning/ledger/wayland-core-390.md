---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: 7e159c955
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
    text: "The per-operation cost of `is_vcs_content_store` does not get worse on a workspace with NO nested checkout, and the price per nested checkout is stated as a number and is flat in the workspace-s directory count."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_guard_cost.rs::the_post_walk_freshness_check_scales_with_checkouts_not_directories"
    owner: core
    note: "MET AFTER REPOINT, and the repoint is the decision rather than a wording tidy. As written the criterion said does not get worse, unqualified. Everything moved DOWN except one figure: the warm guard is unchanged at 3 probes with no nested checkout and 4 on a store-leaf-named path, the slope stays 0, the cold first guard fell 41 -> 36 and vcs_content_stores fell 17.000 -> 5.000 syscalls per directory. The exception is +2 probes per VENDORED CHECKOUT on the admit branch, which is the price of the half of #406 c1 that IS closed -- a real refusal of a real leak. Unqualified, this criterion can only be met by giving that refusal back. Taken deliberately: 2 probes per checkout, flat in directories, is worth a closed leak, and #406 c2 already obliges anyone who moves the figure to re-grade it."
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
