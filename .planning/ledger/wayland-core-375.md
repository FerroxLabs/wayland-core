---
issue: 375
repo: FerroxLabs/wayland-core
kind: defect
title: "GrepTool reads a VCS content store in plaintext when the store's parent is named as the search path"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "Grep(pattern, path='.git') and Grep(pattern, path='.svn') return no bytes from .git/lfs/objects or .svn/pristine under WorkspacePolicy::contained"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D1, found while verifying FerroxLabs/wayland-core#244 — VFS Read of raw .git/objects permitted (compressed; info-only)). Nothing has been done. The measured finding, verbatim: GrepTool reads a VCS content store in plaintext when the CONTROL directory is named one component up. `GrepTool::execute_with_ctx` (crates/wcore-tools/src/grep.rs:78-95) gates only the top-level `path` argument through `ctx.vfs.exists()`, then spawns `rg` directly via `shell_command_argv` (crates/wcore-tools/src/grep.rs:279) — outside BashTool's OS sandbox, so neither SecretDenyFs nor the bwrap fs_read_deny applies to the traversal. `is_vcs_store_dir` matches the store, not its parent, so `.git` and `.svn` pass the guard and rg then descends into `.git/lfs/objects/**` and `.svn/pristine/**`, which hold UNCOMPRESSED file content. MEASURED, not reasoned, in an isolated copy of origin/integ/f13 with the production contained stack (`SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained(root)), root)`): Grep(pattern='CANARY', path='.svn') returned `.svn/pristine/aa/deadbeef.svn-base:1:SVN-CANARY-244 AWS_SECRET_ACCESS_KEY=abc123`; Grep(path='.git') returned `.git/lfs/objects/aa/bb/deadbeef:1:LFS-CANARY-244 password=hunter2`. Controls in the same run: Grep(path='.svn/pristine') and Grep(path='.git/lfs') were both REFUSED ('is a protected secret path'), and Grep(path='.') withheld them ('[Grep policy: 3 match(es) in ignored paths]') — so the deny works and the ignore walk works; it is exactly the name-the-parent shape that bypasses both."
  - id: c2
    text: "A test drives the production contained stack (SandboxedFs over SecretDenyFs over RealFs) for both parent-named spellings and is shown RED against today's code, with the mutation proven to land on executable code"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D1). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The admitted-file decision uses the same predicate the VFS deny uses, so a store reached under any parent name is covered -- not a second name list in grep_policy.rs"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D1). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "The negative controls stay green: Grep(path='.') still withholds ignored matches, and an ordinary in-workspace search is unchanged"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D1). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

GrepTool reads a VCS content store in plaintext when the CONTROL directory is named one component up. `GrepTool::execute_with_ctx` (crates/wcore-tools/src/grep.rs:78-95) gates only the top-level `path` argument through `ctx.vfs.exists()`, then spawns `rg` directly via `shell_command_argv` (crates/wcore-tools/src/grep.rs:279) — outside BashTool's OS sandbox, so neither SecretDenyFs nor the bwrap fs_read_deny applies to the traversal. `is_vcs_store_dir` matches the store, not its parent, so `.git` and `.svn` pass the guard and rg then descends into `.git/lfs/objects/**` and `.svn/pristine/**`, which hold UNCOMPRESSED file content. MEASURED, not reasoned, in an isolated copy of origin/integ/f13 with the production contained stack (`SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained(root)), root)`): Grep(pattern='CANARY', path='.svn') returned `.svn/pristine/aa/deadbeef.svn-base:1:SVN-CANARY-244 AWS_SECRET_ACCESS_KEY=abc123`; Grep(path='.git') returned `.git/lfs/objects/aa/bb/deadbeef:1:LFS-CANARY-244 password=hunter2`. Controls in the same run: Grep(path='.svn/pristine') and Grep(path='.git/lfs') were both REFUSED ('is a protected secret path'), and Grep(path='.') withheld them ('[Grep policy: 3 match(es) in ignored paths]') — so the deny works and the ignore walk works; it is exactly the name-the-parent shape that bypasses both.

**Where.** crates/wcore-tools/src/grep.rs:78-95 (execute_with_ctx gates only the path arg) and :279 (rg spawned outside the sandbox); the admitted-file decision is crates/wcore-tools/src/grep_policy.rs::scope_for / GrepScope::admits. No existing issue: searched FerroxLabs/wayland-core for 'grep content store' and 'Grep .git' across all states — only #242/#243/#234, all Bash-side.

**Why it matters.** This is strictly worse than #244 itself. #244 was filed INFO-only because git loose objects are zlib-compressed; `.svn/pristine` and `.git/lfs/objects` are stored VERBATIM, so this returns a committed secret's plaintext to the model in the contained posture — the posture whose entire premise is that Bash cannot reconstruct a committed secret from the object store. It also falsifies ledger criterion c3 as written, and it is the 'graded the function, not the wiring' shape one level up: the store deny was graded against Read/Write/Edit and BashTool, and nobody counted GrepTool's own subprocess as a call site.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
