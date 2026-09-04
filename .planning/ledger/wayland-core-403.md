---
issue: 403
repo: FerroxLabs/wayland-core
kind: defect
title: "The workspace --lib suite is not ten-times-clean: an ephemeral-port race and three single-sample wall-clock ratios (core#373 c5 remainder)"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "The three `bash::tests` ratio assertions no longer turn on a single wall-clock sample, and the `trusted_local` -> `contained` mutation still reds `a_workspace_that_does_not_walk_cancels_promptly_even_on_a_large_tree` with `cargo check` rc=0 recorded before the red is believed."
    state: not-met
    owner: core
    note: "TEXT RESTORED 2026-09-04 by the post-merge ledger sync, exactly as wayland-core#404 was repaired on 2026-09-03 for the identical transcription defect. All three criteria in this file began at the colon after their bold marker and stopped at the first line-wrap of the issue wrapped bullet, so c1 read : The three `bash::tests` ratio assertions no longer turn on a single -- a fragment nobody can grade in either direction, which is the same failure mode as a gate with no reachable pass state arrived at from the other side. Restored verbatim from the Acceptance section of FerroxLabs/wayland-core#403. THE STATE IS UNCHANGED AND STILL not-met, and was checked against the tree rather than carried over: crates/wcore-tools/src/bash/tests.rs still decides its three ratio assertions against a single measured walk, no cold-sample remedy is present, and no ten-consecutive-run record exists in the tree. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until then: the issue was filed 2026-08-29/30 by this cycle own verification and never entered the release gate, which counts only issues holding a ledger file. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "The two carriers in `crates/wcore-tools/tests/` are either fixed in the same pass or named with a reason they are out of scope."
    state: not-met
    owner: core
    note: "TEXT RESTORED 2026-09-04 by the post-merge ledger sync, exactly as wayland-core#404 was repaired on 2026-09-03 for the identical transcription defect. All three criteria in this file began at the colon after their bold marker and stopped at the first line-wrap of the issue wrapped bullet, so c1 read : The three `bash::tests` ratio assertions no longer turn on a single -- a fragment nobody can grade in either direction, which is the same failure mode as a gate with no reachable pass state arrived at from the other side. Restored verbatim from the Acceptance section of FerroxLabs/wayland-core#403. THE STATE IS UNCHANGED AND STILL not-met, and was checked against the tree rather than carried over: crates/wcore-tools/src/bash/tests.rs still decides its three ratio assertions against a single measured walk, no cold-sample remedy is present, and no ten-consecutive-run record exists in the tree. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until then: the issue was filed 2026-08-29/30 by this cycle own verification and never entered the release gate, which counts only issues holding a ledger file. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "`cargo test --workspace --lib --no-fail-fast` passes N>=10 CONSECUTIVE times on hetzner-dsm with the per-run rc, host load and `never executed` count recorded, and the host load range stated alongside the streak."
    state: not-met
    owner: core
    note: "TEXT RESTORED 2026-09-04 by the post-merge ledger sync, exactly as wayland-core#404 was repaired on 2026-09-03 for the identical transcription defect. All three criteria in this file began at the colon after their bold marker and stopped at the first line-wrap of the issue wrapped bullet, so c1 read : The three `bash::tests` ratio assertions no longer turn on a single -- a fragment nobody can grade in either direction, which is the same failure mode as a gate with no reachable pass state arrived at from the other side. Restored verbatim from the Acceptance section of FerroxLabs/wayland-core#403. THE STATE IS UNCHANGED AND STILL not-met, and was checked against the tree rather than carried over: crates/wcore-tools/src/bash/tests.rs still decides its three ratio assertions against a single measured walk, no cold-sample remedy is present, and no ten-consecutive-run record exists in the tree. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until then: the issue was filed 2026-08-29/30 by this cycle own verification and never entered the release gate, which counts only issues holding a ledger file. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.

All three texts were TRUNCATED on that transcription -- each cut at the first
line-wrap of the issue bullet, leaving a fragment beginning with a colon. They
are restored verbatim on 2026-09-04, the same repair wayland-core#404 received
a day earlier for the same defect. Nothing else about this entry changed: all
three criteria remain not-met, and the restoration only makes them gradeable
at all.
