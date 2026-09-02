---
issue: 322
repo: FerroxLabs/wayland-core
kind: defect
title: "Nested/vendored VCS object stores are not secret-denied (deny walk inspects the workspace root only)"
status: closed
last_verified_commit: 93ede3424
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
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_prunes_a_path_that_resolves_below_a_store_root"
    owner: core
    note: "DISPUTE SETTLED 2026-08-29 (lane f13-fin-hetzner-residuals) BY TEST, NOT BY ARGUMENT -- and THE ADVERSARIAL VERIFIER WAS RIGHT. The lane's parity defence was that the walk PRUNES at the control dir so it can never stand inside a store, making the self-test is_vcs_store_or_control_dir equivalent in effect to the deny walk's ancestor test inside_vcs_store. It is not: pruning governs only what the walk DESCENDS to, while a symlink is an entry met at the TOP of the tree, so one aimed BELOW a store root resolves to a path that is neither a (control, store) shape nor a control-directory leaf. RED ARM, verbatim, on 7390844e with the lane's code unchanged: `thread 'tui::commands::at_ref_resolve::tests::at_dir_prunes_a_path_that_resolves_below_a_store_root' (703687) panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1156:9: / a path resolving BELOW a VCS store root was attached -- the walk's self-test is not the deny walk's ancestor test: [\"blob.txt\", \"shortcut/deadbeef\"]`. TWO leaks, not one: `shortcut` is a directory symlink at <root>/.git/objects/aa (the walk descended and inlined the loose object), and `blob.txt` is a FILE symlink straight at <root>/.git/objects/aa/deadbeef -- the file arm consulted NO store predicate at all, because is_secret_path matches secret NAMES and an object file is named after its hash. So the verifier's narrowing was real and it was WIDER than the criterion text: the file arm was never covered even for a store reached by its own name. THE FIX: is_vcs_store_or_control_dir is renamed is_within_vcs_store_or_control_dir and its store half is now inside_vcs_store -- literally the same predicate WorkspacePolicy::is_vcs_content_store asks -- so the one-list-one-owner claim is now true of the PREDICATE and not only of the constant; the control-directory leaf test stays, because a walk must still stop at .git rather than descend to every object. Both walk arms call it. GREEN, same command: at_dir_prunes_a_path_that_resolves_below_a_store_root ok, and all 6 pre-existing at_ref walk tests still pass, including at_dir_never_walks_a_vcs_store_reached_under_another_name (the lane's own c4 test) and both wrong-refusal controls (gitignore-docs/notes.md and ok.txt attached; the new test additionally requires an ordinary link to an ordinary file to survive, count == 2, so the fix cannot be 'prune every link')."

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
