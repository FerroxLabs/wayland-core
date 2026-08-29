---
issue: 342
repo: FerroxLabs/wayland-core
title: "a_save_during_an_edit_is_not_lost is a real Edit-vs-save data loss, not a load flake"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A guarded write publishes through one atomic name swap, so there is no second observation of the destination left to be stale"
    state: met
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::atomic_write_checked"
    owner: core
    note: "stages a sibling temp file, publishes with renameat2 RENAME_EXCHANGE on Linux and renamex_np RENAME_SWAP on macOS, then reads the temp path which now holds exactly the displaced bytes. Measured at HEAD under load reproducing the reported condition: 0 lost in 2880 attempts across the three edit arms, at windows 2-4x wider than the ones in which the issue measured losses"
  - id: c2
    text: "The displaced bytes are judged by an exact comparison that refuses on any difference"
    state: met
    evidence: "symbol:crates/wcore-tools/src/unsaved_work.rs::pre_image_matches"
    owner: core
    note: "the refusal is reported as changed_under_write. The issue's own recommendation to hoist carry_destination_mode above the check landed in a stronger form - the mode is carried onto the temp file before the publish, so it is not in the window at all"
  - id: c3
    text: "The same guarantee holds on Windows, where the product ships"
    state: not-met
    owner: core
    note: "there is no exchange primitive there - atomic_io.rs:251-254 returns Swap::Unsupported - so the publish degrades to read-then-accept-then-persist, which is the re-check-then-rename design the exchange replaced. #1155 measured that design's residual at about 6.5 percent on the filesystem path and about 70 percent on the vfs path. Nothing tells a Windows user the guarded write is unguarded there"
  - id: c4
    text: "The in-place adversarial arm asserts zero loss rather than tolerating a quarter of the interleavings"
    state: not-met
    owner: core
    note: "inv2_round5_adversarial_test.rs:661 asserts lost*4 < interleaved, which at the measured rate of about 15 interleavings per run passes with up to 3 real losses per 24-attempt run. The exchange makes in-place loss structurally impossible on Linux and macOS, so this is now a gate that cannot fail sitting on the exact regression it was written to catch, and its doc comment still describes the design that was replaced"
  - id: c5
    text: "The two arms asserting a Unix-only guarantee state the Windows truth, from a measured Windows rate"
    state: not-met
    owner: core
    note: "a_save_during_an_edit_is_not_lost and a_save_during_an_edit_is_not_lost_on_the_vfs_path are not cfg-gated and run on the self-hosted Windows leg, asserting lost == 0 - the guarantee the code documents as unavailable there - and 0.13.10 shipped green. Nobody has a Windows loss rate. The sibling wcore-config unit test was split by platform for exactly this reason and these two were not"
---

The issue argues that an Edit overwriting a save which lands mid-operation is a
real data loss, not a load flake, and that the guard's re-check-then-rename
design is the cause.

On Linux and macOS that is fixed and measured. The publish is now a single
atomic exchange, so the check and the publish cannot be split: the temp path
after the swap holds exactly the bytes the publish displaced, and any difference
from what the tool judged refuses and retracts. Under load reproducing the
reported failing condition on hetzner-dsm - 40 CPU burners, 120 concurrent test
processes - all 120 runs passed with zero losses in 2880 attempts, at
interleaving windows two to four times wider than the ones in the report. The
arms are not vacuous; each caught 13 to 21 interleavings per run.

Three residuals keep this open, and they are the reason this file exists.
Windows has no exchange primitive, so it still ships the old design silently
(c3). The in-place arm now tolerates a 25 percent loss rate on a platform where
loss is structurally impossible, so it can no longer report the defect it names
(c4). And two arms assert the Unix guarantee on the Windows CI leg, where either
the race is rare enough to be luck or retries are absorbing it - and nobody
knows which (c5).

Also tracked as #1155 on the wayland tracker, which describes the same defect
from the other side. Criteria come from the cluster E verification note of
2026-08-29.
