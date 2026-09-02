---
issue: 323
repo: FerroxLabs/wayland-core
kind: defect
title: "Two divergent secret denylists: at_ref_guard maintains its own copy, so @-refs miss entries the tools deny"
status: closed
last_verified_commit: 93ede3424
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
    text: "Every name the @-ref attach guard refuses is also carried by the shared denylist the file tools walk, so a name refused to @name is never readable to the model through Read, Grep or Bash"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_guard.rs::every_name_the_attach_guard_refuses_is_carried_by_the_shared_denylist"
    owner: maintainer
    note: "F2 ANSWERED 2026-08-30 (lane f13-atref-guard-b, round 2). PART REFUTED, PART CONCEDED, TEXT NOT TOUCHED AGAIN. REFUTED, the headline: the sentence is NOT unfalsifiable by construction. The guard delegates, but it calls the shared predicate with a DIFFERENT input - is_secret_path anchors a bare @name against a synthetic separator (at_ref_guard.rs:45-51) while Read, Grep and the Bash deny walk pass a real workspace path. Anything that makes the guard refuse a name the shared list does not carry - a local rule re-added, or an anchoring that admits what a real path would not - makes the sentence FALSE. Demonstrated, not modelled: re-adding a local rule to is_secret_path (diff-verified to land on executable code inside the fn, above the delegating call: if path.file_name().is_some_and(|n| n == auth.txt) { return true; }) makes the criterion false. CONCEDED, and this is the real defect the verifier found: the criterion was UNGRADED at the boundary that can falsify it. wcore-tools sits below wcore-cli, so the cited table could never see the guard. Under that same mutation the wcore-tools table stayed GREEN - 'Summary [0.011s] 2 tests run: 2 passed, 1816 skipped' - which is exactly the verifier's point, measured. FIX: new test every_name_the_attach_guard_refuses_is_carried_by_the_shared_denylist (at_ref_guard.rs:333), which quantifies the criterion's own direction over DIVERGENT_SECRET_PATHS + ATTACHABLE_PATHS + a new GUARD_DRIFT_CANARIES table, guard-refuses implies shared-denies-a-real-path, with a wrong-refusal control that the canaries are admitted today. RED ARM 2026-08-30 under the mutation above, verbatim: 'panicked at crates/wcore-cli/src/tui/commands/at_ref_guard.rs:383:9: the @-attach guard refuses these on a rule of its own; Read, Grep and the Bash deny walk consult the shared predicate alone and would still hand them to the model: [\"auth.txt\"]' with 'Summary 75 tests run: 74 passed, 1 failed'. Restored, touched, re-run green. It is corpus-bound and says so in its own doc comment: a local rule on a name in none of the three tables still escapes. SECONDARY POINT, PART REFUTED: the 'never readable through Read, Grep or Bash' clause is NOT graded only by calling the predicate. crates/wcore-tools/tests/git_credentials_denylist_test.rs exercises the real ReadTool, the real GrepTool and the real Bash deny walk against a planted .git-credentials, with wrong-refusal controls on Read and Bash - re-run 2026-08-30: 'Summary [0.038s] 5 tests run: 5 passed, 0 skipped'. CONCEDED residual: that wiring test covers ONE name, so the clause is graded end-to-end for .git-credentials and by the shared predicate plus its enumerated enforcement sites (path_boundary.rs:96, vfs.rs:1734, grep.rs:198, grep_policy.rs:161 and :175, workspace_policy.rs:1827/2062/2680/2694) for the rest. Not a per-name end-to-end proof; recorded rather than claimed. RATIFY: the 2026-08-30 text rewrite below was a LANE decision. Per verifier F4 it needs maintainer ratification before this row counts as a gate. PRIOR NOTE, KEPT: TEXT REWRITTEN 2026-08-29: the old wording presupposed the guard keeps its own file-name check, and 6d130a62 deleted it. Closed by ELIMINATION - the eleven CLI-only names moved into SECRET_BASENAMES / SECRET_NAME_SUFFIXES / SECRET_EXTENSIONS. The dangerous direction is closed too: those names were previously refused to @name and readable to the model through Read/Grep/Bash. Wrong-refusal control: the_shared_denylist_still_admits_ordinary_files. TEXT ACTUALLY REWRITTEN 2026-08-30 (lane f13-atref-guard-b). The 2026-08-29 note above CLAIMED a rewrite that was never performed: the text: field still read 'A rule added to the @-ref guard's own file-name check is also honoured by the tools deny walk'. RE-GRADED at origin/integ/f13 a278f8c3b and the refutation still held: crates/wcore-cli/src/tui/commands/at_ref_guard.rs:52 is `wcore_tools::workspace_policy::is_secret_path_static(for_fragments)` and the file contains no local name list at all (control for that grep: the same query against crates/wcore-tools/src/workspace_policy.rs hits SECRET_BASENAMES at :99), so no rule can be added to a check that does not exist and the old text could never go red. The text above is the property the cited test actually pins, which is the STRONGER one - it is the direction the ticket cared about. RED ARM 2026-08-30, verbatim, deleting '.pgpass' from SECRET_BASENAMES (crates/wcore-tools/src/workspace_policy.rs:107, verified by diff to land on the list entry and not on the comment above it): 'thread 'workspace_policy::tests::the_shared_denylist_carries_every_name_the_at_attach_guard_denies' panicked at crates/wcore-tools/src/workspace_policy/tests.rs:2247:5: the file tools would hand these credential files to the model: [\".pgpass\"]'. Restored and re-run: 'test result: ok. 2 passed; 0 failed'. PRIOR REFUTATION, recorded verbatim: The evidence resolves and is non-vacuous, but the CRITERION TEXT does not hold as written — it is unfalsifiable. Text: 'A rule added to the @-ref guard's own file-name check is also honoured by the tools deny walk.' Commit 6d130a62 DELETED the guard's own file-name check, so no rule can be added to it and the criterion can never go red. The ledger's own note says 'TEXT REWRITTEN 2026-08-29: the old wording presupposed the guard keeps its own file-name check' — but the `text:` field still carries that exact presupposition. The rewrite was claimed, not performed. What IS closed is STRONGER than the text names: `the_shared_denylist_carries_every_name_the_at_attach_guard_denies` (crates/wcore-tools/src/workspace_policy/tests.rs:2240) PASSES, and my mutation drove it red with `the file tools would hand these credential files to the model: ['.pgpass', 'release.keystore', 'deploy_rsa']`. Its 11-row AT_ATTACH_ONLY_SECRETS table pins the dangerous reverse direction the ticket cared about — names refused to `@name` yet readable to the MODEL through Read/Grep/Bash. The substance of #323 is met; the ledger row as recorded is not gradeable and should be rewritten to the property actually pinned before it is called met."
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
eleven CLI-only names it used to carry live in the shared list, pinned by the
11-row AT_ATTACH_ONLY_SECRETS table. Criteria come from the cluster A
verification note of 2026-08-29; every cited symbol and test was re-checked in
this tree.
