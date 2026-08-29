---
issue: 342
repo: FerroxLabs/wayland-core
title: "a_save_during_an_edit_is_not_lost is a real Edit-vs-save data loss, not a load flake"
status: open
last_verified_commit: 43848f75
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
    note: "STALE NOTE CORRECTED 2026-08-29. The Windows primitive DID land in cdf918f6 - publish_displacing now uses ReplaceFileW with lpBackupFileName (atomic_io.rs:327-380) and returns Swap::Displaced - so the previous claim that there is no exchange primitive there is false. It is still not the same guarantee, for three reasons all recorded in-tree: the module's own doc says that unlike RENAME_EXCHANGE there is an instant at which the destination name does not resolve (:230-242); EVERY ReplaceFileW failure degrades SILENTLY to Swap::Unsupported and the old re-check-then-rename fallback (:369-380), including the sharing violation an open editor produces, which is the reported scenario; and the lane self-declares it ungraded on Windows (:244-249), with no Windows run recorded anywhere in the tree."
  - id: c4
    text: "The in-place adversarial arm asserts zero loss rather than tolerating a quarter of the interleavings"
    state: met
    evidence: "test:crates/wcore-tools/tests/inv2_round5_adversarial_test.rs::an_in_place_save_is_not_lost_to_the_final_rename"
    owner: core
    note: "95e0220c. The lost*4 < interleaved tolerance is gone and the vacuity floor interleaved > 0 is kept, so the arm cannot pass by measuring nothing. Re-graded against a recorded run: 12 runs x 24 attempts on hetzner (Linux 6.x, ext4), 230 of 288 saves landing inside the window, 0 lost every run, with Swap::Unsupported named as the red arm."
  - id: c5
    text: "The two arms asserting a Unix-only guarantee state the Windows truth, from a measured Windows rate"
    state: not-met
    owner: core
    note: "PREMISE SHIFTED, MEASUREMENT HALF STILL UNMET. The guarantee is no longer Unix-only now that ReplaceFileW landed, so the two arms no longer assert a Unix-only guarantee - but a_save_during_an_edit_is_not_lost (:630) and a_save_during_an_edit_is_not_lost_on_the_vfs_path (:1051) are both still ungated, both still assert lost == 0, and neither doc comment mentions Windows. No measured Windows rate exists in the tree: no evidence file, no CI artifact, no run log. The instrument that would produce one, atomic_io.rs::the_check_is_handed_the_bytes_the_publish_displaced, is ungated and will run in the Windows CI job, but has never been observed to."
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
