---
issue: 244
repo: FerroxLabs/wayland-core
title: "VFS Read of raw .git/objects permitted (compressed; info-only)"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A VFS read of a raw object store is refused, at the workspace root and at any nested depth"
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_vcs_content_store_deny.rs::nested_object_store_is_refused_by_the_vfs"
    owner: core
    note: "the predicate is WorkspacePolicy::is_vcs_content_store, wired inside SecretDenyFs::guard, which every trait method calls - read, read_pinned, write, exists, list, remove_file, metadata, observe_file and compare_exchange_file"
  - id: c2
    text: "Ordinary repository metadata such as .git/HEAD and .git/refs/heads/main stays readable"
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_vcs_content_store_deny.rs::repository_metadata_stays_readable"
    owner: core
    note: "the wrong-refusal control; without it the suite would be satisfied by a guard that denies the whole of .git"
  - id: c3
    text: "The store is also unreachable to a shell subprocess, not only to the in-process VFS"
    state: not-met
    owner: core
    note: "is_vcs_content_store is enforced by this process, not by a sandbox backend, and vfs_secret_deny_backend_independent.rs deliberately pins that. A Bash tool call can still cat the object files on a host with no working sandbox. Deliberate, but it is the residual and it should be stated rather than assumed away"
---

The issue reports that the VFS Read path permitted raw .git/objects content -
compressed, so it was filed info-only rather than as an exfiltration route.

Its own suggested disposition was that the fix would fall out of applying the
object-store deny to the VFS read path, and that is what happened in the
backlog-sweep merge that preceded v0.13.10. The predicate lives in
WorkspacePolicy, the wiring is inside the single guard every SecretDenyFs
method calls, and the test file names its own red arm - delete the
is_vcs_content_store clause from the guard.

The one residual worth writing down is scope, not correctness: this is an
in-process denial. A subprocess is outside its jurisdiction. Criteria are
transcribed from the cluster A verification note of 2026-08-29; every cited
test was re-checked in this tree.
