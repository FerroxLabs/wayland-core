---
issue: 335
repo: FerroxLabs/wayland-core
kind: defect
title: "@-ref: absolute paths escape the workspace root and skip the gitignore check"
status: open
last_verified_commit: f05b9c9d
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
    text: "The ..-relative escaping spelling is pinned - a git-ignored workspace file stays refused when spelled out through the parent directory"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::a_dotdot_spelling_does_not_escape_the_workspace_gitignore"
    owner: core
    note: "SPLIT 2026-08-29 (lane f13-atref-guard). This row used to claim BOTH escaping spellings on one evidence token, and the absolute half was not pinned - it was covered by an_absolute_path_outside_the_workspace_still_attaches, a wrong-refusal control that passes on both arms. A control is not a pin. The absolute half is now c5; c3 is the ..-relative half alone. Non-vacuous: mutating :219 back to the pre-fix rel_to_root(&full, root) reddens it with 'a git-ignored file must stay refused however it is spelled'. In-fixture control that the plain spelling is genuinely refused first."
  - id: c5
    text: "The absolute escaping spelling is pinned - a git-ignored workspace file stays refused when named by an absolute path whose place inside the root no lexical prefix test can see"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::an_absolute_spelling_does_not_escape_the_workspace_gitignore"
    owner: core
    note: "ADDED 2026-08-29 (lane f13-atref-guard) - the half c3 claimed and did not have. The load-bearing code is canonical_root(root) at :219, and NOTHING graded it: mutating only '&canonical_root(root)' -> 'root' left all 21 shipped at_ref_resolve tests green while the #335 bypass reopened. Two arms in one test: a portable one (a root spelled through a child and back, so strip_prefix cannot see the file is inside it) and a cfg(unix) one on the realistic shape, a symlinked workspace root - the case of every workspace under macOS's /tmp -> /private/tmp. RED ARM, verbatim, mutation confirmed landed on the executable if-let and not the comment above it: 'an absolute spelling of an in-workspace ignored file must stay refused, got Ok(AtPayload { kind: File, files: [ResolvedFile { path: \"/tmp/.tmpPcteAx/build/out.log\", content: \"build log\\n\" }], text: \"\", warnings: [] })'. The a_dotdot_spelling test stayed GREEN on that same arm, which is why the split was needed. Two wrong-refusal controls pass on BOTH arms: the fixture check (spelled against the canonical root) and an absolute attach from genuinely outside the workspace, the capability c1 decided to keep."
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
