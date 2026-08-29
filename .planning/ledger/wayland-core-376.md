---
issue: 376
repo: FerroxLabs/wayland-core
kind: defect
title: "SecretDenyFs rebuilds the VCS content-store list from the filesystem on every ordinary path operation"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The per-operation cost is MEASURED before anything changes: a benchmark or a counted-syscall figure for Read/exists/list/metadata on an ordinary path, recorded on this issue"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D2, found while verifying FerroxLabs/wayland-core#244 — VFS Read of raw .git/objects permitted (compressed; info-only)). Nothing has been done. The measured finding, verbatim: Every SecretDenyFs operation on an ordinary (non-secret) path rebuilds the VCS store list from the filesystem, uncached, and canonicalizes the path twice. `guard` calls `is_project_secret` (which runs `canon_for_scope` — a `canonicalize` syscall) and then, on the common false path, `is_vcs_content_store`, which runs `canon_for_scope` AGAIN and, when arm 1 (lexical) misses — i.e. for every ordinary file — falls through to `vcs_content_stores(&self.root)`: 6x `push_store` (each an `exists()` plus a `canonicalize` when present), a `metadata()` on `<root>/.git` via `gitfile_content_stores`, and a `read_to_string` of `.git/objects/info/alternates` via `alternate_object_dirs`. Roughly 10 filesystem syscalls per Read/exists/list/metadata/write, with no memoisation."
  - id: c2
    text: "If the measurement shows the cost is material, arm 2 of is_vcs_content_store no longer rebuilds the store list on the common path -- either an early-out or the directory-mtime stamp secret_deny_paths_stamped already uses"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D2). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test pins the number of canonicalize/exists calls for one ordinary-path guard, so the regression cannot return silently"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D2). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "If the measurement shows the cost is NOT material, this issue is closed with the figure recorded rather than left open on a code reading"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D2). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

Every SecretDenyFs operation on an ordinary (non-secret) path rebuilds the VCS store list from the filesystem, uncached, and canonicalizes the path twice. `guard` calls `is_project_secret` (which runs `canon_for_scope` — a `canonicalize` syscall) and then, on the common false path, `is_vcs_content_store`, which runs `canon_for_scope` AGAIN and, when arm 1 (lexical) misses — i.e. for every ordinary file — falls through to `vcs_content_stores(&self.root)`: 6x `push_store` (each an `exists()` plus a `canonicalize` when present), a `metadata()` on `<root>/.git` via `gitfile_content_stores`, and a `read_to_string` of `.git/objects/info/alternates` via `alternate_object_dirs`. Roughly 10 filesystem syscalls per Read/exists/list/metadata/write, with no memoisation.

**Where.** crates/wcore-tools/src/vfs.rs:2043 (guard); crates/wcore-tools/src/workspace_policy.rs:935-943 (is_vcs_content_store, arm 2 unconditional on the common path), :2754 (vcs_content_stores), :2871 (gitfile_content_stores), :2903 (alternate_object_dirs), :2935 (canon_for_scope).

**Why it matters.** Stated as a code fact; I did NOT measure the latency, so the impact claim is unproven and should be benchmarked before anyone acts on it. Flagging it because the path is hot — SecretDenyFs is installed unconditionally for every sub-agent (spawner.rs:3023) and for every channel/remote session (channel_tools.rs:185), and sub-agents are read-heavy — and because #244's own issue comment rejected the obvious fix precisely on per-operation syscall cost, so this is the axis the reviewers already said they cared about. A one-line early-out (skip arm 2 when the canonical path is not under root and root has no gitfile/alternates) or caching the store list behind the same directory-mtime stamp `secret_deny_paths_stamped` already uses would remove it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
