---
issue: 335
repo: FerroxLabs/wayland-core
kind: defect
title: "@-ref: absolute paths escape the workspace root and skip the gitignore check"
status: open
last_verified_commit: b437de07
criteria:
  - id: c1
    text: "The policy question - whether an explicitly attached path outside the workspace obeys the workspace gitignore - is decided and written down"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "TAKEN 2026-08-29 as Q1: option A, leave escaping attachments working and pin the behaviour with a test. Recorded in .planning/DECISIONS.md and restated in code at at_ref_resolve.rs:214-218. It is NOT stated in any user-facing doc under docs/, which is the one soft spot."
  - id: c2
    text: "The decision is phrased over paths that escape the root, not over absolute paths"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::canonical_root"
    owner: core
    note: "Phrased over escape, not spelling: resolve_file computes rel_to_root(&admitted.canonical, &canonical_root(root)) at :219, so absolute and ..-relative spellings are judged identically by where they land. Nothing anywhere refuses a path for being absolute."
  - id: c3
    text: "The decided behaviour is pinned by a test covering both escaping spellings, absolute and ..-relative"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::an_absolute_spelling_is_judged_against_the_canonical_workspace_root"
    owner: core
    note: "The ..-relative arm, with an in-fixture control that the plain spelling is genuinely refused first. The absolute arm is an_absolute_path_outside_the_workspace_still_attaches (:951), which pins the decided behaviour rather than a refusal. Both escaping spellings are covered; the one-token rule forced naming one. RE-CLOSED 2026-08-30 (lane f13-atref-guard-b), AS WRITTEN - both escaping spellings are now pinned, not one pin plus one control. The ..-relative arm was and remains a_dotdot_spelling_does_not_escape_the_workspace_gitignore. The ABSOLUTE arm is the evidence test above, and it is new. RE-GRADE at origin/integ/f13 a278f8c3b confirmed the hole was still open: mutating ONLY `rel_to_root(&admitted.canonical, &canonical_root(root))` -> `rel_to_root(&admitted.canonical, root)` left 'test result: ok. 22 passed; 0 failed' across the whole at_ref_resolve suite. The new test hands the resolver a workspace reached through a symlink, so `root` and `canonical_root(root)` are different strings for the same directory, and asserts BOTH absolute spellings of a git-ignored workspace file stay refused; it carries its own in-fixture control that an ordinary file under the same link-reached root still attaches, so it cannot be satisfied by refusing everything. RED ARM 2026-08-30, verbatim, under that same mutation (diff-verified to land on the `if let`, not the comment above it): 'thread 'tui::commands::at_ref_resolve::tests::an_absolute_spelling_is_judged_against_the_canonical_workspace_root' panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1442:13: an absolute spelling of a git-ignored workspace file must stay refused when the root is reached through a link (/tmp/.tmpJPWqY1/real/build/out.log): got Ok(AtPayload { kind: File, files: [ResolvedFile { path: \"/tmp/.tmpJPWqY1/real/build/out.log\", content: \"build log\\n\" }], text: \"\", warnings: [] })'. Restored and re-run: 'test result: ok. 73 passed; 0 failed'. The separate silently-empty-payload defect the sweep found on the @dir walk while verifying this row is FerroxLabs/wayland-core#377, closed in the same branch. PRIOR REFUTATION, recorded verbatim: The named test exists (:970) and is NOT vacuous — I mutated :219 back to the pre-fix `rel_to_root(&full, root)`, confirmed the mutation landed on CODE (the `if let`, not the comment above it) via diff, and it went red with the exact original-bug symptom: `a git-ignored file must stay refused however it is spelled, got Ok(AtPayload { kind: File, files: [ResolvedFile { path: '../repo/build/out.log', content: 'build log/n' }] })`. So the ..-relative half is genuinely pinned. The ABSOLUTE half is not. `an_absolute_path_outside_the_workspace_still_attaches` (:999) is a wrong-refusal CONTROL — the ledger itself says 'Passes on BOTH arms', and I confirmed it stayed green under the full pre-fix mutation. A control is not a pin. The load-bearing absolute-spelling code is `canonical_root(root)`, and it is protected by nothing: mutating ONLY `&canonical_root(root)` -> `root` at :219 leaves ALL 21 shipped at_ref_resolve tests GREEN (`test result: ok. 21 passed; 0 failed`) while the bypass reopens — same fixture returns `Ok(AtPayload{ files:[ResolvedFile{ path: '/tmp/.tmpqv89SA/real/build/out.log', content: 'build log/n' }] })` instead of `Err(GitIgnored(...))`. That is the #335 hole, re-opened, undetected. The genuinely-escaping ..-spelling (E1 above) is likewise unpinned. This is the substituted-property pattern the brief names: c3 asked for a pin over BOTH escaping spellings; the ledger delivered one pin plus one both-arms control and graded itself met."
  - id: c4
    text: "Wrong-refusal controls hold - an in-root gitignored file is still refused and an in-root ordinary file still resolves"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::resolving_a_gitignored_file_is_refused"
    owner: core
    note: "In-root gitignored file still refused. The in-root ordinary file still resolving is pinned by resolve_file_reads_contents_and_reports_token_cost and by an_at_dir_walk_respects_gitignore's kept.txt assertion."
---

An @-ref naming a path outside the workspace root is read without the
workspace's gitignore ever being consulted: resolve_under_root returns the
absolute path unchanged, rel_to_root then fails its strip_prefix, returns None,
and the gitignore branch short-circuits.

The mechanism is confirmed exactly as filed, and the reporter's framing is worth
keeping: this is not privilege escalation. The refs come from the user's own
composer text and they already have read authority over their own filesystem.
The security half is separately closed - after the #323 delegation, secret paths
like ~/.ssh keys and ~/.aws/credentials are refused whether the path is absolute
or not.

One correction changes the fix menu: the issue frames this as an absolute-path
behaviour, and it is not. A ..-relative path escapes the same way. So a fix
phrased over absolute paths would not close the hole it aims at.

What is left is a policy call the lane must not make silently, which is why c1
is blocked on the maintainer. Whatever is chosen should be implemented in the
same change as #339, since #339 rewrites this exact call site. Criteria come
from the cluster A verification note of 2026-08-29.
