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
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_never_walks_a_vcs_store_reached_under_another_name"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-atref-residuals), on its own and NOT folded into #339. The literal file_name == \".git\" test is gone; walk_dir now asks wcore_tools::workspace_policy::is_vcs_store_or_control_dir about the CANONICAL path, so the entry's own name is irrelevant. The new predicate is derived from VCS_CONTENT_STORES - the same list c1/c2 hold - so the walk and the deny walk read one list with one owner; it ORs the existing is_vcs_store_dir shape test with the control-directory leaf (.git/.hg/.svn/.bzr), because a walk PRUNES where the deny walk denies and must stop at the control dir to avoid descending into every object below it. The red arm also proved a SECOND hole the criterion text did not name: the walk skipped .git only, so .hg/store was walked by its own real name. RED ARM, verbatim, before the fix: 'thread ...at_dir_never_walks_a_vcs_store_reached_under_another_name panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1047:9: a VCS content store was walked under another name: [\".hg/store/data/notes.i\", \"mirror/objects/aa/deadbeef\"]' - mirror is a symlink to <root>/.git. Wrong-refusal controls in the same test: an ordinary directory named gitignore-docs and an ordinary root file must both still be attached."
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

The residual was that the same class stayed open on a different surface. The
composer's @-ref directory walk skipped .git by literal name only, which is the
exact shape this issue closed on the tools side. It is recorded here as c4 and
was closed HERE on 2026-08-29, not folded into #339 - #339 rewrote that walk and
shipped past this row, so handing the residual to it a second time would have
retired a live defect. The walk now asks the shared VCS_CONTENT_STORES-derived
predicate about the resolved path.

Criteria come from the cluster A verification note of 2026-08-29; each cited
test was re-checked in this tree.
