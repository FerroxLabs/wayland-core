---
issue: 1298
repo: FerroxLabs/wayland
kind: defect
title: "Signing seeds are published with a non-atomic write: a torn read refuses permanently, and #1250's recorded root cause is refuted by reproduction"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "Both seed writers publish through one helper that stages to a per-call temporary file and links it into place, so no reader can observe a partial seed. Measured by a concurrency test that FAILS against the pre-fix body and passes after, not by inspection."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 while regrading #1250. Anchored at 6e4eca07, which does NOT carry the fix, so every criterion is not-met here BY CONSTRUCTION -- the anchor names the tree the grade was taken on, and grading met against a tree without the fix is the defect this ledger exists to prevent. RED ARM ALREADY RUN, against the verbatim pre-fix body at 6e4eca07 with the new tests grafted on: 16 threads on one fresh path, round 0 of 24, `round 0: concurrent first use refused: receipt is invalid: backend signing seed at /tmp/.tmp78v9pY/container.key is not 32 bytes` -- the production signature verbatim. Green arm on the fix branch: 5 passed / 0 failed. Flip to met with a post-merge sync anchored at the merge commit."
  - id: c2
    text: "Publication is exclusive: N concurrent first-use callers all return the seed that was actually persisted. Measured by the same test asserting every returned seed equals the file's contents."
    state: not-met
    owner: core
    note: "This criterion was earned the hard way and is NOT redundant with c1. The first attempt at the fix used write-then-rename, which is atomic but not EXCLUSIVE: last writer wins the file, so an earlier racer returns a seed the disk does not have and signs with an identity that changes on its next start. Its own concurrency test caught it (`caller 0 returned a seed that is not the persisted one`). Publication is now `hard_link`, which fails with AlreadyExists rather than overwriting."
  - id: c3
    text: "The seed is never reachable under its real name at a mode other than 0600. Measured on unix by reading the published file's mode."
    state: not-met
    owner: core
    note: "The pre-fix order was create-write-chmod, so `fs::write` created the file at the umask default and the private key was world-readable until `set_permissions` ran. The mode is now set on the staging file, before it is reachable under the real name."
  - id: c4
    text: "A corrupt (non-32-byte) seed is still refused rather than silently regenerated, and the refusal names the recovery. Measured by a test asserting the message and that the file survives the refusal."
    state: not-met
    owner: core
    note: "Deliberately NOT self-healing: regenerating would rotate an identity behind the operator's back. But once we can no longer PRODUCE a short file, the error means something else corrupted it, and the operator needs to be told that deleting it is the way back. Before this, a crash mid-write bricked that backend permanently with no stated recovery."
  - id: c5
    text: "The refuted \"state dir removed by a sibling\" attribution is corrected in every file that asserts it, each naming the torn write instead. Graded by a grep returning zero occurrences of the refuted claim, with a control proving the query matches."
    state: not-met
    owner: core
    note: "CORRECTED SCOPE: FOUR files, not six. The refuted ATTRIBUTION -- 'that is the race that failed conformance_matrix at e37e72f0b ... its state dir had just been removed by a sibling' -- appears in tests/conformance_matrix.rs, tests/container_wedge.rs, tests/container_orphan_scan.rs and tests/live_equivalence.rs. src/registry.rs and tests/fail_closed_matrix.rs were re-read and are NOT wrong: they describe the env var redirecting a sibling's record/load/list calls, which is a real and separate hazard, and neither attributes the seed failure. The issue body said six; that was my overcount and it is corrected here rather than forced to fit. A deleted state dir CANNOT emit `is not 32 bytes`: fs::read fails and control falls through to create-and-write. The criterion requires a control because an empty grep reads exactly like a corrected file."
  - id: c6
    text: "The three unguarded fail_closed_matrix.rs tests stop writing into the operator's real config directory during a test run. Graded by a test-env-globals check rather than by inspection."
    state: not-met
    owner: core
    note: "fail_closed_matrix.rs has 13 tests and one StateDirGuard at :564. The tests at :361, :411 and :500 call reference_backends at :364/:414/:502 with no guard, so registry::state_dir() falls through to wayland_config_dir()/exec-backend and all three race the same keys/*.key in the operator's real config dir. Nothing serialises them -- the \"Run serially\" text at :20-21 is prose about a different assertion, and quoting it as a guard would be the doc-comment-as-live-code trap."
---

# The error string is the evidence

`backend signing seed at <path> is not 32 bytes` has exactly one emitter, and reading that function
settles the root cause without running anything: the error is reachable only when the file EXISTS
and reads at a length other than 32. A deleted state dir makes `fs::read` fail, and control falls
through to create-and-write. So the recorded cause -- a sibling removing the state dir -- cannot
produce the signature it was written to explain. A torn write can, and does: 16 threads reproduced
it on the first round against the pre-fix body.

The crate already had the right pattern in `registry::record`, commented "a cancel racing a run must
never read a half file". The two seed writers were the sites that did not get it.
