---
issue: 339
repo: FerroxLabs/wayland-core
title: "SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The secret guard and the read observe the same resolved file identity, so a symlink cannot be graded as one thing and read as another"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_file_refuses_a_symlink_whose_target_is_a_credential_store"
    owner: core
    note: "admit() opens the path ONCE (same_file::Handle - device+inode on Unix, volume-serial+file-index on Windows), canonicalizes, re-opens the canonical name and refuses when the two identities disagree. The guard runs on Admitted::canonical and Admitted::read_to_string consumes that same handle, so nothing re-opens by path after the check."
  - id: c2
    text: "The @dir walk decides recursion on symlink_metadata and computes rel_to_root from the canonical path"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_never_walks_a_symlink_into_a_credential_store"
    owner: core
    note: "MECHANISM SUBSTITUTED, deliberately. The criterion named symlink_metadata plus a canonical rel_to_root; the walk instead canonicalizes each entry and skips anything not under root_canonical (at_ref_resolve.rs:353-365), with a visited set. That permits an in-root dir symlink, which a bare symlink_metadata refusal would have broken - the fix the issue explicitly rejected. The escape is closed and tested; see c6 for the residual the lexical half leaves."
  - id: c3
    text: "read_def_snippet, the fourth read site on this surface, is guarded too"
    state: not-met
    owner: core
    note: "read_def_snippet (at_ref_send.rs:395, reached from render_symbol_blocking:370 via @symbol) still calls std::fs::read_to_string on a repomap-supplied path with no is_secret_path and no admit(). grep -rn read_def_snippet returns only those two lines in the whole tree. The fourth read site is still ungraded."
  - id: c4
    text: "Wrong-refusal controls hold - an in-root symlink to an in-root file still resolves and is still offered by completion"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_file_still_attaches_a_symlink_to_an_ordinary_file"
    owner: core
    note: "The completion half of the control is completion_still_offers_a_symlink_to_an_ordinary_file in at_ref_complete.rs:321. Both pass on the pre-fix arm too, so the suite cannot be satisfied by refusing every symlink."
  - id: c5
    text: "Completion never OFFERS a symlink whose target is a secret, the third production read site on this surface"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_complete.rs::completion_never_offers_a_symlink_to_a_credential_store"
    owner: core
    note: "The issue names three production call sites; the ledger previously carried the completion surface only as a wrong-refusal control. at_ref_complete.rs:113-121 canonicalizes symlinks before judging. Added 2026-08-29 so the row is not missing from the ledger while it is present in the tree."
  - id: c6
    text: "The @dir walk judges gitignore on the resolved path, so an in-root symlink to an in-root gitignored file is not attached"
    state: not-met
    owner: core
    note: "RESIDUAL FOUND WHILE GRADING c2, recorded nowhere else. walk_dir calls rel_to_root(&path, root) on the LEXICAL entry (at_ref_resolve.rs:339) and matches gitignore on it, so <root>/link.txt -> <root>/build/out.log under a build/ rule is judged as link.txt, not ignored, and attached. resolve_file does not have this hole - it uses admitted.canonical at :219. Small, real, and core-owned."
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
