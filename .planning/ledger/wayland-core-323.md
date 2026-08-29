---
issue: 323
repo: FerroxLabs/wayland-core
kind: defect
title: "Two divergent secret denylists: at_ref_guard maintains its own copy, so @-refs miss entries the tools deny"
status: open
last_verified_commit: 6b54e6c2
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
    text: "Every credential name the @-attach guard refuses is also refused by the shared predicate Read/Grep/Bash consult, so a name cannot be denied on one surface and readable on the other"
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::the_shared_denylist_carries_every_name_the_at_attach_guard_denies"
    owner: core
    note: "TEXT REWRITTEN 2026-08-29 (lane f13-atref-guard) - and this time actually performed. The previous text ('a rule added to the @-ref guard's OWN file-name check is also honoured by the tools deny walk') was UNFALSIFIABLE: 6d130a62 deleted that check, so no rule can be added to it and the row could never go red. The row now names the property the cited test really pins, which is stronger: the eleven CLI-only names moved into SECRET_BASENAMES / SECRET_NAME_SUFFIXES / SECRET_EXTENSIONS, and AT_ATTACH_ONLY_SECRETS (workspace_policy/tests.rs:2225) asserts the DANGEROUS direction - names refused to @name yet readable to the MODEL through Read/Grep/Bash. RED ARM, verbatim: blanking the .envrc/.pgpass rows of SECRET_BASENAMES and emptying SECRET_NAME_SUFFIXES (mutation printed before and after; it landed on the const arrays, not the doc comments above them) gave 'the file tools would hand these credential files to the model: [\".envrc\", \".pgpass\", \"deploy_rsa\", \"deploy_ed25519\"]'. Wrong-refusal control the_shared_denylist_still_admits_ordinary_files stayed GREEN on that arm, so the table cannot be satisfied by a predicate that denies everything."
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
