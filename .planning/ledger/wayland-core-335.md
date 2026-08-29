---
issue: 335
repo: FerroxLabs/wayland-core
title: "@-ref: absolute paths escape the workspace root and skip the gitignore check"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The policy question - whether an explicitly attached path outside the workspace obeys the workspace gitignore - is decided and written down"
    state: blocked
    owner: maintainer
    note: "three options with different costs: leave as is and pin it, apply the nearest enclosing repo's gitignore which needs upward discovery that does not exist, or refuse escaping paths which users experience as a regression. This is not privilege escalation - the user already holds read authority over their own files"
  - id: c2
    text: "The decision is phrased over paths that escape the root, not over absolute paths"
    state: not-met
    owner: core
    note: "a relative path escapes identically - @../../secrets/foo.txt joins under the root and then fails rel_to_root on the residual ParentDir, same None, same skipped gitignore, same read. Refusing absolute paths would remove a real capability and leave the hole open"
  - id: c3
    text: "The decided behaviour is pinned by a test covering both escaping spellings, absolute and ..-relative"
    state: not-met
    owner: core
    note: "if the decision is to leave it as is, the test must PIN the behaviour and document it as out of gitignore jurisdiction, so the next auditor does not refile it"
  - id: c4
    text: "Wrong-refusal controls hold - an in-root gitignored file is still refused and an in-root ordinary file still resolves"
    state: not-met
    owner: core
    note: "without these the suite is satisfied by a guard that refuses more than it should, which is how this surface has gone wrong before"
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
