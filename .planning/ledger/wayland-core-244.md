---
issue: 244
repo: FerroxLabs/wayland-core
title: "VFS Read of raw .git/objects permitted (compressed; info-only)"
status: open
last_verified_commit: 43848f75
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
    state: met
    evidence: "test:crates/wcore-tools/tests/bash_vcs_store_deny_linux.rs::a_shell_subprocess_cannot_read_the_vcs_content_store"
    owner: core
    note: "MEASURED 2026-08-29, not reasoned. The previous note assumed the subprocess side was unenforced; it is enforced, and what was actually missing was the grading. The OS path already exists - WorkspacePolicy::secret_deny_paths_stamped extends fs_read_deny with vcs_content_stores(root) for the root store and with vcs_store_entry(..) inside the walk for the #322 nested one - but nothing exercised it end to end, which is the graded-the-function-not-the-wiring shape. The new Linux live-backend test drives the real BashTool: root store .git/objects/ab/cdef and nested vendor/pkg/.git/objects/12/3456 both stay unreadable, with THREE controls - readme.txt reads back (so the sandbox is not refusing everything), .git/HEAD reads back (so the deny is scoped to the CONTENT store and git status is not broken), and the sibling a_recursive_shell_read_cannot_harvest_the_object_store proves grep -r cannot harvest what cat cannot name. RED ARM RUN: with 'out.extend(vcs_content_stores(&self.root))' deleted and vcs_store_entry forced to None, both tests fail - 'the root object store's bytes reached the shell: Exit code: 0 STDOUT: ROOT-OBJECT-BYTES-244'. Restored, touched, 2/2 pass. REMAINING SCOPE, stated not assumed away: on a host with no read-deny-enforcing backend the test skips, because nothing in this process can make a file unreadable to a child there; every non-local-operator principal is refused the shell outright by shell_requires_os_read_deny, and the local-operator carve-out is a taken decision, not an oversight."
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
