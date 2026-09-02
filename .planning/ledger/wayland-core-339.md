---
issue: 339
repo: FerroxLabs/wayland-core
kind: defect
title: "SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it"
status: closed
last_verified_commit: 52b7bc5b
criteria:
  - id: c1
    text: "The secret guard and the read observe the same resolved file identity, so a symlink cannot be graded as one thing and read as another"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_file_refuses_a_symlink_whose_target_is_a_credential_store"
    owner: core
    note: "admit() opens the path ONCE (same_file::Handle - device+inode on Unix, volume-serial+file-index on Windows), canonicalizes, re-opens the canonical name and refuses when the two identities disagree. The guard runs on Admitted::canonical and Admitted::read_to_string consumes that same handle, so nothing re-opens by path after the check."
  - id: c2
    text: "The @dir walk judges every entry by the location it RESOLVES to - it canonicalizes the entry, refuses to descend or inline anywhere outside the union of the canonical workspace root and the canonical directory the reference NAMES, and computes rel_to_root from the canonical path - rather than by the entry's spelling"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::at_dir_never_inlines_an_ordinary_file_from_outside_the_workspace"
    owner: maintainer
    note: "F3 CONCEDED AND TEXT ACTUALLY REWRITTEN 2026-08-30 (lane f13-atref-guard-b, round 2). The verifier was RIGHT: the 2026-08-30 rewrite said the walk 'refuses to leave the resolved root', and the same commit had widened the scope check so it does not. Verified at HEAD: WalkScope::contains is at_ref_resolve.rs:350-352 and reads canonical.starts_with(self.root_canonical) || canonical.starts_with(self.base_canonical), and an_absolute_at_dir_outside_the_workspace_attaches_its_files passes, so the walk demonstrably operates outside the workspace root when the reference names a directory out there. The text: field above is now rewritten to root UNION the named directory, which is the mechanism actually delivered and actually graded by arms B and C. TWO STALE FACTS IN THE NOTE BELOW ARE ALSO CORRECTED HERE rather than silently left: (1) 'skips anything not under root_canonical' is FALSE after this commit - it skips anything not under root OR base; (2) the citation at_ref_resolve.rs:353-365 no longer points at the scope check. The two enforcement sites at HEAD are the directory arm at_ref_resolve.rs:419 (if !scope.contains(&canonical)) and the file arm :475 (if !scope.contains(&admitted.canonical)), both routed through WalkScope::contains at :350-352. The confinement the verifier says is sound - arms B and C, at_dir_never_inlines_an_ordinary_file_from_outside_the_workspace and at_dir_does_not_descend_through_a_link_out_of_the_workspace - is unchanged and still graded; only the recorded sentence was wrong. RATIFY: this text rewrite, and the 2026-08-30 one it corrects, were LANE decisions. Per verifier F4 they need maintainer ratification before this row counts as a gate; the original text named symlink_metadata, which the ticket's own Fix direction forbids in terms, so it could not stand as written either. PRIOR NOTE, KEPT: TEXT REWRITTEN AND SUBSTITUTE GRADED 2026-08-30 (lane f13-atref-guard-b). The old text named symlink_metadata, which this issue's own Fix direction forbids in terms - 'Do not simply refuse all symlinks. Legitimate repos symlink real files; a blanket refusal breaks ordinary use and will get switched off' - so it could not be satisfied without reopening the thing the ticket rejected. It is rewritten above to the mechanism that WAS delivered, which is the stronger one for this issue's identity-not-name concern. MECHANISM AS DELIVERED. The criterion named symlink_metadata plus a canonical rel_to_root; the walk instead canonicalizes each entry and skips anything not under root_canonical (at_ref_resolve.rs:353-365), with a visited set. That permits an in-root dir symlink, which a bare symlink_metadata refusal would have broken - the fix the issue explicitly rejected. The escape is closed and tested; see c6 for the residual the lexical half leaves. UNTIL 2026-08-30 THE SUBSTITUTE WAS UNGRADED. Re-graded at origin/integ/f13 a278f8c3b: mutating either scope line to a constant left the whole suite green - `if !canonical.starts_with(root_canonical) {` -> `if false {` gave 'test result: ok. 22 passed; 0 failed', and `if !admitted.canonical.starts_with(root_canonical)` -> `if false` gave the same. The previously cited evidence test at_dir_never_walks_a_symlink_into_a_credential_store survives both because its fixture target is NAMED .git-credentials, so is_secret_path alone satisfies it: it grades the name denylist, not the confinement. Two tests now grade the two lines separately, each with an in-fixture control that the ordinary file beside the link still attaches. FILE ARM (the evidence above) - an in-root link to an ordinary, NON-denylisted file outside the workspace, so no name rule can save it. RED ARM verbatim: 'thread 'tui::commands::at_ref_resolve::tests::at_dir_never_inlines_an_ordinary_file_from_outside_the_workspace' panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1471:9: the @dir walk inlined a file from outside the workspace through an in-root link'. DIRECTORY ARM - at_dir_does_not_descend_through_a_link_out_of_the_workspace. Its effect is not a leak (the file arm catches the bytes); it is that the walk never LEAVES the workspace, so the observable is the skip count: the link is ONE skipped entry, not the six files behind it. RED ARM verbatim: 'panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1506:9: assertion `left == right` failed: the walk descended through a link out of the workspace - it weighed the six files behind the link instead of skipping the link itself: AtPayload { kind: Dir, files: [ResolvedFile { path: \"ok.txt\", content: \"safe\\n\" }], text: \"\", warnings: [SkippedFiles { count: 6 }] } / left: Some(6) / right: Some(1)'. Both mutations were diff-verified to land on executable code. Restored and re-run: 'test result: ok. 73 passed; 0 failed'. The separate FIFO wedge the sweep found on this walk while verifying this row is FerroxLabs/wayland-core#381, closed in the same branch. PRIOR REFUTATION, recorded verbatim: REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: Two halves. The second half ('computes rel_to_root from the canonical path') IS met — at_ref_resolve.rs:374 and :412 both call rel_to_root against root_canonical. The first half ('decides recursion on symlink_metadata') is NOT met; the ledger says so openly ('MECHANISM SUBSTITUTED, deliberately') and the substitution is defensible, because the ticket explicitly forbids the blanket symlink refusal symlink_metadata would produce. The finding is that the SUBSTITUTE is completely ungraded. I mutated the substitute away twice in the scratch copy and nothing noticed: M5, at_ref_resolve.rs:357 'if !canonical.starts_with(root_canonical) {' -> 'if false {' gave 'test result: ok. 65 passed; 0 failed'; M6, at_ref_resolve.rs:400 'if !admitted.canonical.starts_with(root_canonical)' -> 'if false' also gave 'test result: ok. 65 passed; 0 failed'. The cited evidence test at_dir_never_walks_a_symlink_into_a_credential_store survives both because its fixture target is NAMED .git-credentials, so is_secret_path(&admitted.canonical) alone satisfies it — the test grades the name denylist, not the scope confinement the ledger's own note points at ('skips anything not under root_canonical'). I confirmed the guard does work at HEAD with a throwaway probe (in the scratch copy, deleted after): an in-root symlink notes.txt -> <outside>/taxes.txt containing PRIVATE-OUTSIDE-PAYLOAD printed 'PROBE_ESCAPE leaked=false files=['ok.txt']'. So the behaviour is correct and the code is right; what is missing is any test that would go red if someone deleted either scope line. Remainder: one test — @dir must not inline an out-of-workspace, non-denylisted file reached through an in-root symlink."
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
