---
issue: 1181
repo: FerroxLabs/wayland
kind: defect
title: "Four orphaned lane branches carry unmerged fixes, two of them in the 'a check that ran nothing' class"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "lane/walk-parallel has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: met
    evidence: "file:.planning/ORPHAN-LANE-DISPOSITIONS-2026-08-29.md:31:## lane/walk-parallel — 13a81ab8 — SUPERSEDED by addb4f48"
    owner: core
    note: "OUTCOME 2026-08-29: SUPERSEDED by addb4f48 (release v0.13.5). Recorded in full at the cited section. Evidence is the RECORD and not `commit:addb4f48` deliberately: CI checks out with fetch-depth 1, so no commit-shaped token resolves there - see the note on the tracker-wide instance of this at the bottom of the record. This repo squash-merges, so --is-ancestor, git diff and git cherry all report 'absent' for work that is present; the test used was reverse application (git show <sha> | git apply --reverse --check) plus a symbol grep for every commit that did not reverse-apply. 13a81ab8 reverse-applies; 13cc6ef5 and b024b922 are present under later edits (SERIAL_WALK_BUDGET, walk_root_is_covered, tests/walk_parallel_identity_test.rs, refined by 92ee5374/d1f55f0b/620ddc79). NOT RUBBER-STAMPED - this is the 'assertion that cannot fail' branch, so its own stated red arm was re-run on hetzner: the node_modules prune inserted into the PARALLEL closure alone at workspace_policy.rs:2574 (asserted to land on CODE, not on the four node_modules mentions in the comment block at :2483-2492) gives '5 tests run: 3 passed, 2 failed' - deny_set_is_complete_and_identical_on_the_parallel_arm and the_parallel_arm_returns_exactly_what_the_serial_arm_returns, the latter with 'only in serial: [\"node_modules/vendor/deep/client.pem\"]'. Exactly the two 13a81ab8 predicted. Restored, touched, 5/5 pass. Tip archived at origin/archive/lane-walk-parallel-superseded."
  - id: c2
    text: "lane/winpath has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: met
    evidence: "file:.planning/ORPHAN-LANE-DISPOSITIONS-2026-08-29.md:65:## lane/winpath — 4089798c — SUPERSEDED by addb4f48"
    owner: core
    note: "OUTCOME 2026-08-29: SUPERSEDED by addb4f48 (release v0.13.5). Recorded in full at the cited section. 4089798c reverse-applies cleanly. Its two siblings are present by symbol: http_client.rs:157 pub async fn awaiting_first_byte with all four tests from 7d8a8a8b (a_fast_dispatch_does_not_fire_the_connect_silence_signal, an_established_stream_that_goes_quiet_still_emits_exactly_one_notice, a_stalled_dispatch_surfaces_a_silence_signal_before_the_connect_timeout, the_silence_threshold_must_beat_the_connect_deadline) and wcore-skills/src/paths.rs::normalize_path_separators. Tip archived at origin/archive/lane-winpath-superseded."
  - id: c3
    text: "lane/tools-bash has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: met
    evidence: "file:.planning/ORPHAN-LANE-DISPOSITIONS-2026-08-29.md:79:## lane/tools-bash — c7aeaf2d — SUPERSEDED by addb4f48"
    owner: core
    note: "OUTCOME 2026-08-29: SUPERSEDED by addb4f48 (release v0.13.5). Recorded in full at the cited section. None of the three commits reverse-applies - all three are present under later edits, verified by symbol: bash.rs:419 LOSSY_OUTPUT_NOTE, :430 decode_lossy, :446 drain_lines, :496 spawn_manifest_build, and c7aeaf2d's named build-timeout cause ('the workspace secret-scan); the command never ran') at both arms, :994 and :1156. This is why reverse-apply alone is not the whole test."
  - id: c4
    text: "lane/win-fix has a recorded outcome: rebased and merged, superseded by a named commit, or closed as obsolete"
    state: met
    evidence: "file:.planning/ORPHAN-LANE-DISPOSITIONS-2026-08-29.md:89:## lane/win-fix — c5ce3857 — SUPERSEDED by 9150ff1f"
    owner: core
    note: "OUTCOME 2026-08-29: SUPERSEDED by 9150ff1f (release v0.12.26-rc.1, #257). Recorded in full at the cited section, which carries the per-commit table. Eleven substantive commits graded individually; a9be1214 and 82455bd6 reverse-apply, the other nine are present by symbol - justfile [unix]/[windows] test-ci split at :44-68, ci.yml 'Assert this leg produced test signal' at :591 and :991, config.rs::home_alone_isolates_on_unix_and_does_not_isolate_on_windows, snapshot.rs::set_hostile_file_dacl, the four soak-script functions, MacIdentityRecheck, PROBE_TIMEOUT 90s, the TMPDIR 'overridden' retain. NOT RUBBER-STAMPED - this is the 'CI ran zero tests' branch, so the gate was EXERCISED, not read: the ci.yml:591 step extracted verbatim gives exit=1 with '::error NO TEST SIGNAL' on a missing junit, exit=1 with '::error ZERO TESTS' on a junit declaring tests=0, and exit=0 on a junit declaring 13350 - a reachable fail in both dark modes AND a reachable pass. RESIDUAL: the tip could NOT be archived to origin - it carries .github/workflows/ci.yml and the push is rejected for want of workflow scope on hetzner's token (the same limit MASTER-PLAN.md records against #1177), as a branch and as a tag. Its content is in origin/main via 9150ff1f, so a box loss costs the history and not the work; archiving the ref needs a workflow-scope token (maintainer)."
  - id: c5
    text: "lane/finish-a and lane/finish-b, named in the issue as unpushed branches that would orphan on a box loss, have landed"
    state: met
    evidence: "commit:d92e61d1"
    owner: core
    note: "Added 2026-08-29; the issue's trailing ask had no criterion. d92e61d1 merges lane/finish-a and 883b2504 merges lane/finish-b into integ/next, so the box-loss risk is discharged. NOTE the same class is live again: lane/session-tickets is twelve commits ahead of integ/next and unmerged."
---

Four `lane/*` branches are not ancestors of main and were never merged or closed
out. Each carries a real fix, and two of them fix failure classes this repo
treats as worst: an assertion that could not fail for the thing it named, and a
green check that ran nothing.

They are 6 to 27 days stale against bases that have moved a long way, so merging
them unverified would be the same mistake in the other direction. Each needs
three answers before it moves: does the defect still exist on current main, is
the fix still correct against the current tree, and does its test still go red
under the mutation it was written for.

One criterion per branch, because the acceptance is explicitly per branch and
three outcomes are allowed for each. None of the four tips resolves in this
worktree's remotes, so nothing here is evidence that any of them has been
handled — only that this checkout cannot see them.
