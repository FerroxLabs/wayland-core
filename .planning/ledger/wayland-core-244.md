---
issue: 244
repo: FerroxLabs/wayland-core
kind: defect
title: "VFS Read of raw .git/objects permitted (compressed; info-only)"
status: open
last_verified_commit: e7cb6679
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
    evidence: "test:crates/wcore-tools/tests/grep_vcs_store_deny.rs::naming_the_git_dir_does_not_return_lfs_object_content"
    owner: core
    note: "REFUTED AS WRITTEN 2026-08-29 and RE-MET the same day. The criterion says A shell subprocess, unqualified; the cited BashTool test only covers BashTool. GrepTool spawns rg/grep/findstr ITSELF via shell_command_argv (grep.rs), outside SecretDenyFs AND outside the OS sandbox, and gated only the top-level path argument through ctx.vfs.exists. is_vcs_content_store matches the STORE, not its parent, so naming the CONTROL directory one component up made the store the walk root - where the ignore crate hidden-file filter has nothing left to hide - and the backend descended into it. MEASURED under the production contained stack SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained(root)), root): Grep(pattern=CANARY, path=.svn) returned .svn/pristine/aa/deadbeef.svn-base:1:SVN-CANARY-244 AWS_SECRET_ACCESS_KEY=abc123 and Grep(path=.git) returned .git/lfs/objects/aa/bb/deadbeef:1:LFS-CANARY-244 password=hunter2. Strictly worse than the #244 the ticket opens with: .svn/pristine and .git/lfs/objects are stored VERBATIM, not zlib-compressed, so this returned a committed secrets PLAINTEXT. CLOSED by two guards, one per spelling class - grep_policy::scope_for now prunes the ignore walk at is_vcs_store_dir (at the STORE, never at the control directory, so .git/HEAD stays searchable), rebuilding each candidate on the canonical walk root so a symlink alias cannot change the answer; and run_grep refuses a target that IS a store, which is the only guard the context-free execute() entry point has. RED ARM RUN: with crates/wcore-tools/src/ checked out at the pre-fix e7cb6679b and touched, 6 of the 8 tests in grep_vcs_store_deny.rs fail, quoting the leaks verbatim; restored, touched, 8/8 pass and wcore-tools is 1824/1824. The BashTool half below still holds and its evidence is test:crates/wcore-tools/tests/bash_vcs_store_deny_linux.rs::a_shell_subprocess_cannot_read_the_vcs_content_store. PRIOR NOTE, still true: MEASURED 2026-08-29, not reasoned. The previous note assumed the subprocess side was unenforced; it is enforced, and what was actually missing was the grading. The OS path already exists - WorkspacePolicy::secret_deny_paths_stamped extends fs_read_deny with vcs_content_stores(root) for the root store and with vcs_store_entry(..) inside the walk for the #322 nested one - but nothing exercised it end to end, which is the graded-the-function-not-the-wiring shape. The new Linux live-backend test drives the real BashTool: root store .git/objects/ab/cdef and nested vendor/pkg/.git/objects/12/3456 both stay unreadable, with THREE controls - readme.txt reads back (so the sandbox is not refusing everything), .git/HEAD reads back (so the deny is scoped to the CONTENT store and git status is not broken), and the sibling a_recursive_shell_read_cannot_harvest_the_object_store proves grep -r cannot harvest what cat cannot name. RED ARM RUN: with 'out.extend(vcs_content_stores(&self.root))' deleted and vcs_store_entry forced to None, both tests fail - 'the root object store's bytes reached the shell: Exit code: 0 STDOUT: ROOT-OBJECT-BYTES-244'. Restored, touched, 2/2 pass. REMAINING SCOPE, stated not assumed away: on a host with no read-deny-enforcing backend the test skips, because nothing in this process can make a file unreadable to a child there; every non-local-operator principal is refused the shell outright by shell_requires_os_read_deny, and the local-operator carve-out is a taken decision, not an oversight."
---

The issue reports that the VFS Read path permitted raw .git/objects content -
compressed, so it was filed info-only rather than as an exfiltration route.

Its own suggested disposition was that the fix would fall out of applying the
object-store deny to the VFS read path, and that is what happened in the
backlog-sweep merge that preceded v0.13.10. The predicate lives in
WorkspacePolicy, the wiring is inside the single guard every SecretDenyFs
method calls, and the test file names its own red arm - delete the
is_vcs_content_store clause from the guard.

The residual written down here was scope, not correctness: the VFS denial is
in-process, and a subprocess was said to be outside its jurisdiction. That
framing hid the actual hole. It is not only the operator's shell that is a
subprocess - GrepTool spawns one of its own, and that one is not sandboxed at
all, so "outside its jurisdiction" meant "unguarded" rather than "guarded
elsewhere". Enumerating the model-facing tools that spawn a process, rather
than the two named instances, is what found it.

The enumeration is every tool in wcore-tools whose `execution_class_for`
returns `ProcessSpawning`, which is seven:

| tool | how it spawns | this criterion |
|---|---|---|
| `BashTool` | the immutable session sandbox | covered (Linux) by bash_vcs_store_deny_linux.rs |
| `AwsCliTool`, `GcloudTool`, `KubectlTool` | the SAME session sandbox, explicitly "exactly as BashTool" per their module docs | inherit the same `fs_read_deny` |
| `ScriptTool` | spawns nothing itself — dispatches other allow-listed built-ins | inherits whatever they enforce |
| `GrepTool` | `shell_command_argv("rg"/"grep"/"findstr")`, NO sandbox | LEAKED |
| `GitTool` | `shell_command_argv("git")`, NO sandbox | LEAKED |

Two of the seven spawn outside the sandbox, and both were the two nobody had
graded. They leaked when measured:

* `GrepTool` — naming the control directory one component up. Closed by the
  walk prune and the store refusal; graded by grep_vcs_store_deny.rs.
* `GitTool` — `Git{op: diff, rev: HEAD~1, path: ".env"}` returned
  `-AWS_SECRET_ACCESS_KEY=PROBE-GIT-9931` with `.env` deleted from the working
  tree, so its bytes lived only in the object store. `blame` returned the same
  line. Closed by a secret-path refusal on both ops plus per-file section
  withholding on a whole-tree `diff`; graded by
  crates/wcore-tools/tests/git_secret_content_test.rs. Its red arm ran: with
  crates/wcore-tools/src/git.rs at the pre-fix e7cb6679b and touched, 4/4 fail
  quoting the leaks; restored, touched, 4/4 pass.

Two of those four GitTool arms were VACUOUS when first written and passed
against the pre-fix tree — `blame` failed for "no such path" because the
fixture had deleted the file, and a pure rename emits no content hunk to
withhold. Both were rewritten so the op would otherwise succeed, and only then
did the red arm grade the guard. That correction is recorded because the same
mistake is what this criterion's history is made of.

What is still NOT graded here: the whole class is Linux/unix-measured. The
GrepTool and GitTool guards are platform-independent (in-process, no
`cfg` split), but no macOS or Windows run has exercised them.
