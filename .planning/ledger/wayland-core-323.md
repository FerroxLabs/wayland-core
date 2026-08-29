---
issue: 323
repo: FerroxLabs/wayland-core
kind: defect
title: "Two divergent secret denylists: at_ref_guard maintains its own copy, so @-refs miss entries the tools deny"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The @-ref attach guard delegates to the shared secret predicate instead of maintaining a parallel copy of it"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::is_secret_path"
    owner: core
    note: "The guard's own list is DELETED. is_secret_path anchors a relative path with a leading separator (the shared rules match separator-anchored fragments such as /.ssh/, and it anchors against a synthetic root, not the process CWD) and delegates every rule to wcore_tools::workspace_policy::is_secret_path_static."
  - id: c2
    text: "A table refuses every path either denylist carries, so it goes red if either list loses an entry"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::the_attach_guard_denies_every_path_either_denylist_carries"
    owner: core
    note: "29 rows, each historically carried by only one of the two lists. Both halves now live in wcore-tools, so the table goes red if the merged list loses any of them."
  - id: c3
    text: "A wrong-refusal control pins that ordinary attachable paths are still offered"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::ATTACHABLE_PATHS"
    owner: core
    note: "includes notes/turnkey.json and docs/monkey.json to pin the separator boundary of the *-key.json rule, so the table cannot be satisfied by a guard that denies everything"
  - id: c4
    text: "Every name the @-ref attach guard refuses is carried by the shared denylist the file tools walk, so a name refused to @name is never readable to the model through Read/Grep/Bash"
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::the_shared_denylist_carries_every_name_the_at_attach_guard_denies"
    owner: core
    note: "TEXT ACTUALLY REWRITTEN 2026-08-29 (lane f13-n-atref). The previous rewrite was claimed in this note but never performed: the text: field still read 'A rule added to the @-ref guard's own file-name check is also honoured by the tools deny walk', which 6d130a62 made UNFALSIFIABLE by deleting that check - no rule can be added to a check that does not exist, so the criterion could never go red. The text above is now the property the cited test actually pins, which is the stronger one. Closed by ELIMINATION - the eleven CLI-only names moved into SECRET_BASENAMES / SECRET_NAME_SUFFIXES / SECRET_EXTENSIONS. The dangerous direction is closed too: those names were previously refused to @name and readable to the model through Read/Grep/Bash. Wrong-refusal control: the_shared_denylist_still_admits_ordinary_files."
---

The issue reports two secret denylists that had drifted: the composer's @-ref
attach guard kept its own copy, so an @-ref could attach a file the tools would
refuse to read.

The guard now delegates to the shared predicate, which makes the drift the
issue names structurally impossible in that direction. What makes the fix
trustworthy rather than merely plausible is the table behind it: 29 paths, each
denied by exactly one of the two lists, so the test goes red if either list
loses an entry - which is precisely the property whose absence let them diverge
- plus a real set of attachable controls so it cannot be passed by refusing
everything.

c4 records the reverse direction, and it is closed rather than residual: the
guard owns no local file-name rule any more (6d130a62 deleted it), and the
eleven CLI-only names it used to carry live in the shared list, pinned by an
11-row AT_ATTACH_ONLY_SECRETS table. Criteria come from the cluster A
verification note of 2026-08-29; every cited symbol and test was re-checked in
this tree.
