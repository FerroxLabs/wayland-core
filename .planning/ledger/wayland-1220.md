---
issue: 1220
repo: FerroxLabs/wayland
kind: defect
title: "A cleared flaky-allowlist entry came back through a merge, and nothing in this repo can detect a resurrected line"
status: open
last_verified_commit: 70a47aaed
criteria:
  - id: c1
    text: ".config/flaky-allowlist.txt on the integration branch no longer carries the gh#1182 line"
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::contained_construction_does_not_walk_the_workspace"
    owner: core
    note: "MET AS WRITTEN, verified against the MERGED TREE (origin/integ/f13 at 70a47aaed) rather than against a commit hash. The line is gone: `grep -n contained_construction .config/flaky-allowlist.txt` returns nothing while the known-positive control `grep -c 2026 .config/flaky-allowlist.txt` returns 8 on the same file, so the absence is measured by a query that can see a hit. The needle is the TEST NAME, not the `gh#1182` tag, so re-listing the same test under any other owner is still caught."
  - id: c2
    text: "The allowlist is graded against the MERGED tree rather than against a commit hash: a check refuses an entry whose owning ledger criterion claims it was deleted"
    state: met
    evidence: "file:scripts/check-criteria-ledger.py:492:m = ABSENT_EV.match(ev)"
    owner: core
    note: "MET AS WRITTEN. The `absent:<path>::<text>` evidence kind re-reads the file on EVERY gate run, so a resurrection reds the ledger instead of surviving it; the path must exist, which is the known-positive control (an absence check over a renamed file fails loudly rather than passing forever). wayland#1182 c3 carries `absent:.config/flaky-allowlist.txt::contained_construction_does_not_walk_the_workspace`. LIVE RED ARM on the real tree, exit codes captured directly: `python3 scripts/check-criteria-ledger.py --offline` exits 0 at HEAD; appending the gh#1182 line back to .config/flaky-allowlist.txt makes the SAME command exit 1 with 'wayland-1182.md:21: c3 evidence does not resolve -- .config/flaky-allowlist.txt still contains ...'; restored with `git checkout --` + `touch` and it exits 0 again."
  - id: c3
    text: "The check is proven in both directions, including a resurrection introduced by a MERGE -- git log -S skips merges by default, which is how this one passed"
    state: met
    evidence: "file:scripts/check-criteria-ledger.py:1243:def _merge_resurrection(resolution):"
    owner: core
    note: "MET AS WRITTEN. `--self-test` now BUILDS the history rather than describing it: a base holding the entry plus an untouched neighbour, a deletion in the c461293f shape, a lane branch cut BEFORE the deletion that rewords the same lines, and the merge back. `-X theirs` resurrects the entry and the gate REDS naming the criterion; `-X ours` does not and it stays GREEN -- the two arms differ in nothing else, so the red is the resolution. The reproduction is verified rather than asserted: a third arm runs `git log -S <needle>` on that same merged tree and requires it to find the ordinary commits and NOT report the merge, so if `-S` ever stops being blind to it this stops being a reproduction and says so. RED ARM on the enforcement site: guarding `if m.group('n') in t` with `if False and ...` turns 'MERGE resurrected the entry' from RED to green and the self-test exits 1 with 'self-test: BROKEN'; restored, `--self-test` exits 0."
---

A flaky-allowlist entry that was deliberately deleted (commit c461293f, 'clear three fixed flaky-allowlist entries') was silently resurrected by merge commit 9c9f27b0 'Merge remote-tracking branch origin/lane/f13-fix-shared-lib into integ/f13'. `git diff 9c9f27b0^1 9c9f27b0 -- .config/flaky-allowlist.txt` shows the gh#1182 line coming back as a `+`. The other two entries the same commit cleared (dangerous_lease_e2e_test x2) did NOT come back, so this is a partial merge-resolution regression, not a wholesale revert.

**Where.** .config/flaky-allowlist.txt:59 at origin/integ/f13; introduced by merge 9c9f27b0; original deletion in c461293f

**Why it matters.** The retry-flake gate (.github/scripts/grade-retry-flakes.sh, a REQUIRED context on main) now silently tolerates FLAKY retries of `wcore-tools::workspace_policy::tests::contained_construction_does_not_walk_the_workspace` until 2026-10-15. If that test starts flaking for a NEW reason — its own file explains it is a security-boundary instrument control — the run conclusion stays SUCCESS and nothing names it. More generally, nothing in the repo detects a resurrected allowlist line: `git log -S` skips merges by default, which is exactly how this passed the lane's own check. Every future 'cleared the allowlist entry' claim graded off a commit hash rather than the merged tree has the same hole.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
