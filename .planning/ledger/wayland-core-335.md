---
issue: 335
repo: FerroxLabs/wayland-core
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
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::a_dotdot_spelling_does_not_escape_the_workspace_gitignore"
    owner: core
    note: "The ..-relative arm, with an in-fixture control that the plain spelling is genuinely refused first. The absolute arm is an_absolute_path_outside_the_workspace_still_attaches (:951), which pins the decided behaviour rather than a refusal. Both escaping spellings are covered; the one-token rule forced naming one."
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
