---
issue: 339
repo: FerroxLabs/wayland-core
kind: defect
title: "SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it"
status: open
last_verified_commit: f05b9c9d
criteria:
  - id: c1
    text: "The secret guard and the read observe the same resolved file identity, so a symlink cannot be graded as one thing and read as another"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_file_refuses_a_symlink_whose_target_is_a_credential_store"
    owner: core
    note: "admit() opens the path ONCE (same_file::Handle - device+inode on Unix, volume-serial+file-index on Windows), canonicalizes, re-opens the canonical name and refuses when the two identities disagree. The guard runs on Admitted::canonical and Admitted::read_to_string consumes that same handle, so nothing re-opens by path after the check."
  - id: c2
    text: "The @dir walk stays inside the workspace - an in-root symlink at a file outside the root is not attached, and one at a directory outside the root is not descended into"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_never_reaches_outside_the_workspace_through_a_symlink"
    owner: core
    note: "TEXT REWRITTEN 2026-08-29 (lane f13-atref-guard). The old text named a MECHANISM - symlink_metadata - that the issue explicitly forbids implementing, because a bare symlink_metadata refusal breaks the in-root dir symlink ordinary repos use. The walk instead canonicalizes each entry and skips anything not under root_canonical (:357 dir branch, :400 file branch), with a visited set. That substitute was graded by NOTHING: replacing either check with 'if false' left all 65 at_ref tests green, because the old evidence test's fixture is NAMED .git-credentials and is_secret_path alone satisfies it. The new test plants nothing on any denylist, so only the scope check keeps it out. RED ARMS, verbatim, both mutations printed in context to confirm they landed on the if-conditions and not the comments above them: dir branch -> 'the walk descended through a symlink out of the workspace: [\"alias.txt\", \"escape/back.txt\", \"ok.txt\"]'; file branch -> 'a file outside the workspace was inlined through an in-root symlink: [\"alias.txt\", \"notes.txt\", \"ok.txt\"]'. The directory half needs its own observable because a file outside the root is refused by the FILE check even when the walk descends, so the fixture links from the outside tree back to an in-root file. Wrong-refusal control in the same test - an in-root link stays attached - passes on BOTH arms. The issue's own reproduction, at_dir_never_walks_a_symlink_into_a_credential_store, still passes and is retained; it is now known to be satisfied by this scope check rather than by the denylist, which is why it could not serve as this row's pin."
  - id: c3
    text: "read_def_snippet, the fourth read site on this surface, is guarded too"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_send.rs::symbol_snippet_refuses_a_symlink_to_a_credential_store"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-atref-residuals). read_def_snippet now calls at_ref_resolve::read_guarded - the lexical floor, then admit() (open once, canonicalize, re-open and compare same_file::Handle), then is_secret_path on the RESOLVED name, then the bytes from that same handle. The guard is a shared helper in at_ref_resolve rather than a copy here, because two copies of a guard that must agree are how this surface grew four read sites with three answers. RED ARM, verbatim, before the fix: 'thread ...symbol_snippet_refuses_a_symlink_to_a_credential_store panicked at crates/wcore-cli/src/tui/commands/at_ref_send.rs:772:9: the @symbol preview followed a link into a credential store: https://user:s3cr3t-token@git.example.com', and the direct arm 'symbol_snippet_refuses_a_credential_store ... the @symbol preview inlined a credential store: https://user:s3cr3t-token@git.example.com'. Wrong-refusal control symbol_snippet_still_previews_an_ordinary_source_file (at_ref_send.rs) PASSED on the pre-fix arm too, so the pair cannot be satisfied by refusing every read. Both symbol tests green after."
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
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_judges_gitignore_on_the_resolved_path"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-atref-residuals). walk_dir still applies the cheap lexical rel first, and now ALSO judges the rule on rel_to_root(resolved, root_canonical) - admitted.canonical in the file branch, fs::canonicalize in the dir branch - so a link named around a rule is judged by what it resolves to, matching resolve_file (core#335). No extra syscall: the file branch reuses the canonicalization admit() already performs, and the dir branch reuses the one the scope check already performs. RED ARM, verbatim, before the fix: 'thread ...at_dir_judges_gitignore_on_the_resolved_path panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:996:9: a git-ignored file was attached through a link named around the rule: [\".gitignore\", \"alias.txt\", \"notes.txt\", \"ok.txt\"]'. The test carries its own wrong-refusal control in the same assertion set - a link to a NON-ignored file must still be attached (count == 2) - so the fix cannot be to skip every link."
  - id: c7
    text: "The @dir walk judges the secret denylist on what each entry RESOLVES to, so an in-root link at an in-root secret is not attached"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_judges_the_secret_denylist_on_the_resolved_path"
    owner: core
    note: "ADDED 2026-08-29 (lane f13-atref-guard), found by enumerating the walk's guards while closing c2. at_dir_never_walks_a_symlink_into_a_credential_store LOOKS like it pins this and does not - its .git-credentials sits OUTSIDE the root, so the scope check refuses it before the denylist is consulted. Deleting is_secret_path(&admitted.canonical) from the walk left all 67 at_ref tests green. The reachable shape is an IN-root link at an IN-root secret, where the scope check has nothing to say. RED ARM, verbatim, mutation printed in context: 'the @dir walk inlined a secret reached through a link named around the denylist: [\"alias.txt\", \"notes.txt\", \"ok.txt\"]'. Wrong-refusal control in the same test passes on BOTH arms."
  - id: c8
    text: "The @dir walk judges a directory-only gitignore rule on the resolved path, so an ignored tree reached under another name is not walked"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_judges_a_directory_gitignore_rule_on_the_resolved_path"
    owner: core
    note: "ADDED 2026-08-29 (lane f13-atref-guard). c6's test covers the FILE branch only; the DIRECTORY branch's rel_to_root(&canonical, root_canonical) check at :374 could be replaced by 'if false' with all 67 at_ref tests green, and nothing downstream catches the entries - is_ignored returns early for a dir_only rule asked about a file, so a 'build/' tree reached as 'docs' is walked and every file under it inlined. RED ARM, verbatim, mutation printed in context: 'a git-ignored directory was walked through a link named around the rule: [\".gitignore\", \"docs/out.txt\", \"lib/main.rs\"]'. Wrong-refusal control - an ordinary directory reached through a link is still walked, walked ONCE because of the visited set - passes on BOTH arms."
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
