---
issue: 335
repo: FerroxLabs/wayland-core
kind: defect
title: "@-ref: absolute paths escape the workspace root and skip the gitignore check"
status: open
last_verified_commit: 43848f75
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
    note: "RE-CLOSED 2026-08-29 (lane f13-n-atref). The ..-relative arm was genuinely pinned by a_dotdot_spelling_does_not_escape_the_workspace_gitignore; the ABSOLUTE arm was not. The one named for it, an_absolute_path_outside_the_workspace_still_attaches, is a wrong-refusal CONTROL that passes on both arms, and the load-bearing absolute-spelling code - the canonical_root(root) in the rel_to_root at :219 - was protected by nothing: mutating ONLY &canonical_root(root) -> root left all 21 shipped at_ref_resolve tests green while the bypass reopened. The evidence test above closes that. It hands the resolver a workspace reached through a symlink, so root and canonical_root(root) are different strings for the same directory, and asserts that BOTH absolute spellings of a git-ignored workspace file stay refused. RED ARM, verbatim, under that exact mutation: 'panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1369:13: an absolute spelling of a git-ignored workspace file must stay refused when the root is reached through a link (/tmp/.tmp6DrEGW/real/build/out.log): got Ok(AtPayload { kind: File, files: [ResolvedFile { path: \"/tmp/.tmp6DrEGW/real/build/out.log\", content: \"build log\\n\" }], text: \"\", warnings: [] })'. It carries its own in-fixture control (an ordinary file under the same link-reached root still attaches), so it cannot be satisfied by refusing everything."
  - id: c4
    text: "Wrong-refusal controls hold - an in-root gitignored file is still refused and an in-root ordinary file still resolves"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::resolving_a_gitignored_file_is_refused"
    owner: core
    note: "In-root gitignored file still refused. The in-root ordinary file still resolving is pinned by resolve_file_reads_contents_and_reports_token_cost and by an_at_dir_walk_respects_gitignore's kept.txt assertion."
  - id: c5
    text: "An @dir whose spelling escapes the lexical root attaches its tree, rather than resolving to a silently empty payload"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::a_dotdot_at_dir_attaches_the_tree_and_still_obeys_the_gitignore"
    owner: core
    note: "ADDED 2026-08-29 (lane f13-n-atref) for defect D3, found verifying c3. c1's decision - leave escaping attachments working - was delivered for @file and NOT for @dir: walk_dir named every entry with rel_to_root(&path, root) and dropped the None case, so EVERY escaping spelling resolved to Ok(AtPayload { kind: Dir, files: [], text: \"\", warnings: [] }) - no error, no warning, and no skipped count either, because the drop did not increment it. The user saw a successful chip and the model received nothing. Fixed by separating the two roots walk_dir had conflated (symbol:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::WalkScope): the workspace root is the .gitignore's JURISDICTION, the directory the reference names is what the walk may not LEAVE. The absolute arm is an_absolute_at_dir_outside_the_workspace_attaches_its_files. RED ARM, verbatim, on origin/integ/f13 before the fix and again under a mutation restoring the pre-fix drop: 'an escaping `@dir` spelling resolved to a silently empty payload: AtPayload { kind: Dir, files: [], text: \"\", warnings: [] }' and 'an absolute `@dir` outside the workspace resolved to an empty payload: AtPayload { kind: Dir, files: [], text: \"\", warnings: [] }'. The test also pins the core#335 property for the walk: a .. spelling that resolves back INTO the workspace still obeys its .gitignore."
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
