---
issue: 323
repo: FerroxLabs/wayland-core
title: "Two divergent secret denylists: at_ref_guard maintains its own copy, so @-refs miss entries the tools deny"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The @-ref attach guard delegates to the shared secret predicate instead of maintaining a parallel copy of it"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::is_secret_path"
    owner: core
    note: "it checks its own file-name rules and then calls wcore_tools::workspace_policy::is_secret_path_static. The separator anchoring before that call is load-bearing - the shared list matches anchored fragments like /.ssh/, and it anchors against a synthetic root, not the process CWD"
  - id: c2
    text: "A table refuses every path either denylist carries, so it goes red if either list loses an entry"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::the_attach_guard_denies_every_path_either_denylist_carries"
    owner: core
    note: "29 rows, each denied by exactly one of the two lists, including the issue's own .hg/hgrc example. Removing the anchoring silently drops 14 of the 19 shared-list paths and only this table catches it"
  - id: c3
    text: "A wrong-refusal control pins that ordinary attachable paths are still offered"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::ATTACHABLE_PATHS"
    owner: core
    note: "includes notes/turnkey.json and docs/monkey.json to pin the separator boundary of the *-key.json rule, so the table cannot be satisfied by a guard that denies everything"
  - id: c4
    text: "A rule added to the @-ref guard's own file-name check is also honoured by the tools deny walk"
    state: not-met
    owner: core
    note: "is_secret_file_name lives only in wcore-cli. Delegation closed divergence in one direction - a new shared-list entry now reaches @-refs automatically - but a rule added on the CLI side still does not reach the tools. The residual is narrower than the filed defect, not absent"
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

The honest residual is the reverse direction, recorded as c4: the guard still
owns a local file-name rule the tools do not see. Criteria come from the
cluster A verification note of 2026-08-29; every cited symbol and test was
re-checked in this tree.
