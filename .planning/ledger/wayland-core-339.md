---
issue: 339
repo: FerroxLabs/wayland-core
title: "SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The secret guard and the read observe the same resolved file identity, so a symlink cannot be graded as one thing and read as another"
    state: not-met
    owner: core
    note: "is_secret_path is documented as purely lexical and there is zero symlink resolution anywhere on the @-ref surface - no canonicalize, symlink_metadata, read_link or is_symlink in the guard, resolve, complete or send modules. ln -s ~/.git-credentials notes.txt then @notes.txt returns the credential bytes"
  - id: c2
    text: "The @dir walk decides recursion on symlink_metadata and computes rel_to_root from the canonical path"
    state: not-met
    owner: core
    note: "path.is_dir() follows symlinks, so a link to a directory is recursed into, and rel_to_root is strip_prefix on the lexical path so nothing prunes it. A link named docs pointing at HOME gets the home directory walked and inlined, bounded only by DIR_MAX_FILES. That is materially worse than the one credential file the issue describes"
  - id: c3
    text: "read_def_snippet, the fourth read site on this surface, is guarded too"
    state: not-met
    owner: core
    note: "reached from @symbol, it calls read_to_string on a repomap-supplied path with no is_secret_path call at all. The issue enumerates three call sites; four exist. Lower reachability, but ungraded is ungraded"
  - id: c4
    text: "Wrong-refusal controls hold - an in-root symlink to an in-root file still resolves and is still offered by completion"
    state: not-met
    owner: core
    note: "mandatory, not optional. Without them the suite is satisfied by a guard that refuses every symlink, which is the explicitly rejected fix - legitimate repos symlink real files"
---

The composer's @-ref guard grades a path as a string. It never resolves it. So
a symlink named notes.txt pointing at a credential file matches nothing on the
denylist and is then followed by the read that comes after the check.

This is the only genuinely open defect in its cluster, and the issue understates
it in two ways. The @dir walk escalates the same trick from one file to an
entire external tree, because the directory test follows links and the
root-relative check that would prune an escape is computed lexically and so
never sees one. And the .git skip is by literal name, so a link called anything
else pointing at an object store is walked - the same class #322 closed on the
tools side, on a surface that has no equivalent guard.

The fix shape is one change, not four: resolve once, guard the opened handle's
identity, and read from that handle rather than re-opening by path. #335 shares
this exact call site and its decision should land with it. A reusable
canonicalization helper already exists in wcore-tools but is crate-private;
making it public is the cheap route.

Criteria come from the cluster A verification note of 2026-08-29, which
confirmed the claim exactly as filed against the shipped tree.
