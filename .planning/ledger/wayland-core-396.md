---
issue: 396
repo: FerroxLabs/wayland-core
kind: defect
title: "A BARE repository vendored under the root has its object store VFS-readable: no arm of is_vcs_content_store sees it (class remainder of #390)"
status: open
last_verified_commit: 30fd6cfde
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
    note: "Same as #394 c3: the 5 syscalls/directory figure was measured on a tree that is not an ancestor of `integ/f13`, and the probe named in the criterion is not in this lineage. A probe was written here and the comparable measurement made base-vs-HEAD on the same fixture: 35.009 -> 34.997 syscalls per traversed directory, known-positive control green in every run."
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
