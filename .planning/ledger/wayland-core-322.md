---
issue: 322
repo: FerroxLabs/wayland-core
title: "Nested/vendored VCS object stores are not secret-denied (deny walk inspects the workspace root only)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A vendored or nested VCS object store is refused at any depth, not only directly under the workspace root"
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_vcs_content_store_deny.rs::every_vcs_store_shape_is_refused_at_any_depth"
    owner: core
    note: "is_vcs_store_dir is a lexical last-two-component shape test over VCS_CONTENT_STORES, which is now held as (dir, leaf) pairs so the root-relative join and the any-depth test cannot drift apart"
  - id: c2
    text: "The deny walk emits the store directory itself, never its members, so the fix does not cost a canonicalize per object"
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_vcs_content_store_deny.rs::nested_store_reaches_the_os_deny_list"
    owner: core
    note: "this is the trap the issue explicitly warned about - thousands of entries plus a canonicalize each. The test also carries an empty-list control"
  - id: c3
    text: "Ordinary repository metadata stays readable at every depth"
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_vcs_content_store_deny.rs::repository_metadata_stays_readable"
    owner: core
    note: "the wrong-refusal control the issue demanded; .hg/dirstate and .svn/wc.db stay readable too"
  - id: c4
    text: "The TUI @-ref directory walk gives the same treatment to a store reached under another name"
    state: not-met
    owner: core
    note: "NARROWED BY #339 BUT STILL OPEN, and NOT superseded: #339 shipped without touching it, so handing the residual to an issue that has already passed over it would retire a live defect. The 6d130a62 walk prunes a link pointing at a store OUTSIDE the workspace (canonicalize + starts_with(root_canonical), at_ref_resolve.rs:353-365), but a directory or symlink inside the root under any other name still canonicalizes inside and is walked. at_ref_resolve.rs:350 still skips a directory only when its file_name is exactly .git."
---

The issue reports that the secret deny walk classified VCS object stores only
at the workspace root, so a vendored or submodule store one level down was
readable.

It is fixed, and fixed the way the issue directed - a directory classifier
folded into the existing deny walk rather than a new rule in the flat secret
path list. Both walk arms are wired, serial and parallel; fixing only one would
have been the classic miss and both were checked. The test file names its own
red arms and carries the wrong-refusal controls, so it cannot be satisfied by a
guard that refuses all of .git.

The residual is that the same class is still open on a different surface. The
composer's @-ref directory walk skips .git by literal name only, which is the
exact shape this issue closed on the tools side. It is recorded here as c4 and
is being fixed as part of #339, which rewrites that walk.

Criteria come from the cluster A verification note of 2026-08-29; each cited
test was re-checked in this tree.
