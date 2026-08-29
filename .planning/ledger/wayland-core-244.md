---
issue: 244
repo: FerroxLabs/wayland-core
kind: defect
title: "VFS Read of raw .git/objects permitted (compressed; info-only)"
status: open
last_verified_commit: a278f8c3
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
    evidence: "test:crates/wcore-tools/tests/grep_vcs_store_deny.rs::grep_cannot_harvest_a_vcs_content_store_named_one_level_up"
    owner: core
    note: "CLOSED 2026-08-30 against the counterexample that refuted it, not against an adjacent property. The refutation reproduced FIRST, at integ/f13 a278f8c3b, with the test file below added and NO fix: `Grep(pattern, path = \".git\")` returned `.git/lfs/objects/aa/bb/deadbeef:1:WLCANARY-LFSOBJ-244` and `.git/objects/ab/cd1234:1:WLCANARY-ROOTOBJ-244`, and `Grep(path = \"vendor/x/.git\")` returned `vendor/x/.git/objects/12/3456:1:WLCANARY-NESTED-322` -- PLAINTEXT, under the production `SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained(root)), root)` stack. `ctx.vfs.exists()` gates only the TOP-LEVEL `path` argument and `.git` is not itself a store, so naming the control directory ONE COMPONENT UP walked in while naming the store outright was refused. THE FIX is a new lexical predicate `workspace_policy::is_vcs_content_store_static` (the same any-depth arm `WorkspacePolicy::is_vcs_content_store` tries first, not a second copy) asked at BOTH grep_policy decision points -- the `WalkBuilder` admission in `scope_for` and the explicit-file gate in `GrepScope::admits` -- plus a `vcs_store` row in the withholding footer so the refusal is reported, never silent. It sits in `grep_policy`, the ONE place all three backends (`rg`, POSIX `grep`, `findstr`) are held to the same answer, so the class is closed for the backend the host happens to have. RED ARM, hetzner, tree committed first: M1 `is_vcs_content_store_static` -> `{ let _ = path; false }` (verified on executable lines 2186-2189, not a doc comment) reddened 2 of 5 -- `grep_cannot_harvest_a_vcs_content_store_named_one_level_up` and `grep_cannot_harvest_a_nested_vcs_content_store` -- with the literal store bytes in the panic message. M2 -> `{ let _ = path; true }` (blanket deny) reddened the OTHER two, the wrong-refusal controls: `an_ordinary_file_is_still_searchable` got `[Grep policy: 6 match(es) in VCS content stores withheld]` and the `.git/HEAD` control got `[Grep policy: 3 match(es) in VCS content stores withheld]`. Restored byte-identical (`git diff` empty), touched, 5/5 green. Also RUN ON WINDOWS 10.0.26200.9168 (5/5 pass), which the platform-scoped Linux sibling never covered. SUBPROCESS CLASS ENUMERATED, 3 families in wcore-tools: (1) GrepTool, 3 backends through one policy point -- fixed here; (2) GitTool, which reads the object store BY DESIGN via blame/diff/log -p and is already recognised in-tree and dropped from Full channel-remote posture (channel_tools.rs:107-112, `FULL_CHANNEL_DENY`), a taken decision rather than a gap -- see c5's note; (3) tirith_security, which lints a COMMAND STRING and never traverses a path. `.git/HEAD` and `.git/refs` stay searchable, which is c2's carve-out held."
  - id: c4
    text: "The OS-sandbox half holds: a Bash subprocess cannot read the store at the root or at any nested depth"
    state: met
    evidence: "test:crates/wcore-tools/tests/bash_vcs_store_deny_linux.rs::a_shell_subprocess_cannot_read_the_vcs_content_store"
    owner: core
    note: "Split out of c3 on 2026-08-30 because the schema allows one evidence token per criterion and c3 needed the token that resolves its own refutation. This half was MEASURED 2026-08-29 and re-confirmed: the test drives the real BashTool under bwrap, root store `.git/objects/ab/cdef` and nested `vendor/pkg/.git/objects/12/3456` both unreadable, with three controls (readme.txt reads back, `.git/HEAD` reads back, and the recursive sibling proves `grep -r` cannot harvest what `cat` cannot name). SCOPE, stated not assumed away: `#![cfg(target_os = \"linux\")]` and it skips where `platform_enforces_read_deny()` is false, so on Windows -- where the job-object default confines nothing -- it skips by construction. GitTool remains a deliberate exception on the LOCAL contained path: channel_tools.rs:107-112 records that it reads git-TRACKED content straight from the object store and that under the STRICT sandbox it is the ONLY door to a branch, a push or a pull request, so it is dropped from channel/remote Full posture and kept locally. That is a taken product decision, not an ungraded gap."
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
