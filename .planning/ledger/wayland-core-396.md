---
issue: 396
repo: FerroxLabs/wayland-core
kind: defect
title: "A BARE repository vendored under the root has its object store VFS-readable: no arm of is_vcs_content_store sees it (class remainder of #390)"
status: open
last_verified_commit: 967bdf2fb
criteria:
  - id: c1
    text: "A VFS `Read` of an object under a BARE repository vendored beneath the"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_bare_repository_vendored_under_the_root_is_refused
    owner: core
    note: "MEASURED. `vfs_nested_store_deny.rs::a_bare_repository_vendored_under_the_root_is_refused` covers BOTH spellings the ticket names -- `<root>/vendor/pkg.git/objects/ab/cd1234` and the suffix-less `<root>/vendor/mirror/objects/ab/cd1234` -- through the production stack. Sibling working-tree control (`vendor/notes.md`) readable, and the repository's own `HEAD` stays readable, mirroring the `git rev-parse` carve-out. Closed by arm 3 (`encloses_repository_store`), which asks whether an ancestor carrying a store leaf name has a parent that IS a repository (`HEAD` plus `refs` or `config`) rather than what the path is called; a bare repo has no control directory for a name-based arm to find."
  - id: c2
    text: "Whatever detects a bare repository is graded against a NEGATIVE control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::an_ordinary_directory_named_like_a_store_is_not_a_repository
    owner: core
    note: "`an_ordinary_directory_named_like_a_store_is_not_a_repository`, WIDENED past the criterion: not only `objects` but every `VCS_CONTENT_STORES` leaf name -- `objects`, `modules`, `store`, `lfs`, `pristine`, `repository` -- under `<root>/app/<leaf>/index.ts`, with no `HEAD`/`refs` anywhere. All six readable through the stack AND `is_vcs_content_store` asserted false for each, so the fix cannot be a bare component match. This is also the test that would catch a `store_shaped`-style lexical denial being introduced later."
  - id: c3
    text: "The per-traversed-directory cost `grep_policy::scope_for` pays does not"
    state: not-met
    owner: core
    note: "MEASURED base-vs-HEAD on hetzner, base `ca15a48bf` vs HEAD `967bdf2fb` (lane/f13-s2-vcs-cost). Two DISTINCT binaries, identity proven by content and not by date: sha256 differs and `strings base_bin | grep -c nested_declarations_moved` = 0 against 5 for HEAD.\nDifferential `strace -f -c` over `workspace_policy::tests::probe_vcs_content_stores_per_traversed_directory`, SAME fixture both arms (a root `.git` plus WL_PROBE_DIRS ordinary `pkg{i}/main.rs` directories), arms INTERLEAVED at every configuration so host-load drift hits both, THREE operation counts (WL_PROBE_DIRS 100/1100/2100) x WL_PROBE_REPS 1/6. The probe`s own known-positive control (`1 passed` and `scope_for` still classifying the root store) asserted green in all 12 runs.\nDifferencing REPS 6 against REPS 1 at the same directory count cancels arm 4`s one-off walk, which is itself O(directories) and is otherwise indistinguishable from a per-traversal cost:\n  base 100->1100  steady 34.998 syscalls/traversed dir   (one-off 18.988)\n  base 1100->2100 steady 35.000                          (one-off 19.084)\n  HEAD 100->1100  steady 35.001                          (one-off 19.039)\n  HEAD 1100->2100 steady 35.001                          (one-off 19.012)\nSo this change moves the figure by <= 0.003 syscalls/directory, which is below the run-to-run spread of the instrument itself (a second full interleaved pass earlier in the same session read 35.001 on BOTH arms).\nTHE 5 SYSCALLS/DIRECTORY FIGURE IS NOT REPRODUCIBLE ON THIS LINEAGE AND THE CRITERION CANNOT BE MET AS WRITTEN. It was measured at `875bf32cb` on `lane/f13-w3-vcs-residuals`; `git merge-base --is-ancestor 875bf32cb HEAD` is FALSE, as it is for `972d1c17c`. That lineage`s arm 3 gated a whole-tree walk on a path spelling; this one (`0ed5d4707`, an ancestor - verified YES) resolves stores eagerly into a `OnceLock`-style set and its `grep_policy::scope_for` asks `denies_read_content`. Different traversal, different fixture, different tree: the two numbers are not comparable, and 5.000 is not a bar this code can be held to. TWO INDEPENDENT LANES HAVE NOW MEASURED ~35 HERE (lane/f13-vcs-store read 35.009 -> 34.997; this lane reads 34.998-35.001 across three operation counts and two passes), which is the reproducibility the 5.000 lacks.\nPROPOSED REPOINT, to be made ON THE ISSUE and not here: replace `does not exceed the 5 syscalls/directory measured at #390`s merge` with `does not exceed 35.1 syscalls per traversed directory on the probe_vcs_content_stores_per_traversed_directory fixture (a root .git plus WL_PROBE_DIRS ordinary directories), re-measured base-vs-HEAD, interleaved, with the probe`s known-positive control green in every run`. The fixture is named because the previous figure`s unreproducibility is exactly a fixture difference."
  - id: c4
    text: "`Grep` and the VFS agree on the shape: a test asserts the point"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::grep_and_the_vfs_agree_about_a_bare_repository_object
    owner: core
    note: "`grep_and_the_vfs_agree_about_a_bare_repository_object`: the same bare-repo object is refused by the VFS and absent from `Grep(pattern, path=\".\")`, with Grep's own positive control (the ordinary file's match) asserted present so a broken Grep cannot pass. Structurally tied rather than duplicated -- `grep_policy::scope_for` asks this policy's `denies_read_content`, which is the same conjunction `SecretDenyFs::guard` asks, so arms 3 and 4 reach Grep without a second name list."
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
