---
issue: 1295
repo: FerroxLabs/wayland
kind: defect
title: "main is red: every squash-merged lane orphans its ledger last_verified_commit, and the release path can never see it"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Every ledger entry's `last_verified_commit` resolves to a commit that is an ancestor of the tree being shipped, so a reader of a release can re-derive each grading from the history they actually have."
    state: met
    evidence: "file:scripts/check-criteria-ledger.py"
    owner: core
    note: "MET by this change. BEFORE, on main at 93ede3424 at full history: FAIL, 183 problem(s) -- the same count CI reports for step 12 of run 33581626099. ALL 183 ledger files were affected, not a subset (measured: already-ancestor = 0), so the break was total. Cause is the merge strategy, not a bad value: an entry recorded the LANE commit it was graded against, the lane was SQUASH-merged, and that commit is never an ancestor of main. Two flavours, which the gate distinguishes: 174 were `not an ANCESTOR of HEAD` (object present via a fetched lane branch), 9 were `not a commit in this tree` (lane branch deleted) -- all 9 being 67fa14db6, i.e. wayland-1284..1291 and wayland-core-416, filed 2026-09-01 and genuinely wrong. FIX: re-anchor each entry to `git log -1 HEAD -- <that ledger file>`, the commit on main that carries it, an ancestor by construction. AFTER: RC=0 at full depth. I CONSIDERED AND REJECTED the alternative of relaxing the gate to accept a non-ancestor object that still resolves, because the gate's own comments record that ancestry was chosen DELIBERATELY over existence after a measurement: twelve entries citing be4467ed passed `cat-file -t` locally and failed all twelve in CI, making the local gate weaker than the CI gate in exactly the direction that lets a bad pointer ship. Relaxing it would reinstate that. TWO COSTS, both measured and both stated rather than hidden. (1) All 185 anchors now read 93ede3424, because #417's squash touched every ledger file, so the field is REDUNDANT with `git log -1 -- <file>` until the next lane lands and they diverge again. I checked whether that makes the check vacuous and it does NOT: the NEGATIVE CONTROL still reds -- corrupting wayland-core-401.md's anchor to deadbeef1 gives RC=1 with the correct message, and restoring gives RC=0. (2) The lane sha is no longer structured data. It is not lost: the old value of every one of the 183 entries is in this commit's own diff, one `git show` away, and many entries also cite it in their `note:` prose."
  - id: c2
    text: "A red `main` is visible to whoever is about to cut a release, rather than being discoverable only by someone who goes looking."
    state: not-met
    evidence: "file:.github/workflows/release.yml"
    owner: maintainer
    handoff: "FerroxLabs/wayland#1295"
    note: "OPEN, and the more serious half. main's CI has been red for four consecutive commits since 2026-08-29: 93ede3424 failure (CI linux-containerized + report), b26e4058d failure (report only, fixed in #417), bc13e6e32 failure (CI macos-latest) -- which is the v0.13.11 TAG -- and 20d990061 failure. v0.13.11 was tagged and published off a red main and nothing surfaced it. Note the release path cannot see c1 at all: the arm is armed ONLY in ci.yml, which sets fetch-depth: 0 deliberately (ci.yml:1459-1471); release.yml's prepare-release checkout (release.yml:43) is depth-1, where the script self-skips with a NOTE and returns 0. Measured both ways on the same tree: depth-1 -> RC=0 with the SKIP notice, full -> RC=1 with 183 problems."
  - id: c3
    text: "A branch taken from main can pass the required `report` check, so main is not merge-locked."
    state: not-met
    evidence: "file:.github/workflows/ci.yml"
    owner: core
    note: "OPEN until a green run proves it, deliberately NOT claimed on the strength of c1. Because step 12 red on every tree descended from the squash, `CI (linux-containerized)` failed, `report` aggregated that failure, and `report` is a required context -- so nothing could merge to main. CONFIRMED INDEPENDENTLY rather than only on my own tree: lane/toolsearch-miss-scope (compare main...lane: ahead_by 1, behind_by 0, i.e. branched from current main) failed the SAME step in run 33584638843, which is somebody else's lane, so the lock was a property of main and not of anything I constructed. c1 removes ONE of the two reasons ci-linux was red. The other is step 27, tracked as #1296 and NOT reproduced or understood yet, so this criterion cannot be graded met until a run of this branch shows `report` green."
---

# The anchors were orphaned by the merge strategy, and the gate that says so cannot reach the release

Every ledger entry anchored to the lane commit it was graded against. Squash-merging
destroys that commit, so on `main` the anchor resolved to nothing in the history being
shipped. All 183 entries were in that state.

The gate was right to say so. c1 fixes the data rather than weakening the gate, and states
the semantics it chooses. c2 -- that a red `main` reached a published tag with nobody
seeing it -- is untouched. c3 stays open until a green run proves the merge lock is
actually gone, because #1296 is the other half of that red and is not yet understood.
