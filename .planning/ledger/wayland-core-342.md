---
issue: 342
repo: FerroxLabs/wayland-core
kind: defect
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
    state: superseded
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::publish_displacing"
    owner: core
    handoff: "FerroxLabs/wayland-core#370"
    note: "MEASURED FALSE 2026-08-29, and carried to FerroxLabs/wayland-core#370 with the numbers. The previous note reasoned from the source that the Windows guarantee was weaker; it is now observed to be weaker, which is a different claim and a stronger one. On SEANDESKTOP (Windows 11 build 26200) at this tree, twelve `--retries 0` executions of each arm: the filesystem arm lost 7 saves of the 169 that landed inside the window (4.1%) and the vfs arm 1 of 144 (0.7%); 11 of the 24 executions were red. The full tally is on c5 and in the two arms' doc comments. The two named mechanisms, both still in the tree: `atomic_io.rs:369-380` degrades EVERY `ReplaceFileW` failure - the sharing violation an open editor produces included, which is the reported scenario - to `Swap::Unsupported` and the old re-check-then-rename fallback, silently, with no log and no counter; and separately the competing save can be refused outright with `Os { code: 5, kind: PermissionDenied }` while the guard holds the destination open, which is not loss but is not the Unix guarantee either. #370 carries the contract (make the degrade observable or refuse; then either close the gap or write the weaker Windows guarantee down as the shipped contract) and the acceptance (the two arms at retries=0 over N>=20, or gated with the measured rate and a separate arm grading the declared weaker guarantee, plus a negative control proving the degrade path is observable when it fires)."
  - id: c4
    text: "The in-place adversarial arm asserts zero loss rather than tolerating a quarter of the interleavings"
    state: met
    evidence: "test:crates/wcore-tools/tests/inv2_round5_adversarial_test.rs::an_in_place_save_is_not_lost_to_the_final_rename"
    owner: core
    note: "95e0220c. The lost*4 < interleaved tolerance is gone and the vacuity floor interleaved > 0 is kept, so the arm cannot pass by measuring nothing. Re-graded against a recorded run: 12 runs x 24 attempts on hetzner (Linux 6.x, ext4), 230 of 288 saves landing inside the window, 0 lost every run, with Swap::Unsupported named as the red arm."
  - id: c5
    text: "The two arms asserting a Unix-only guarantee state the Windows truth, from a measured Windows rate"
    state: met
    evidence: "test:crates/wcore-tools/tests/inv2_round5_adversarial_test.rs::a_save_during_an_edit_is_not_lost"
    owner: core
    note: "MEASURED AND WRITTEN INTO BOTH ARMS 2026-08-29. The rate, on SEANDESKTOP (Windows 11 build 26200), tree ab6b602f, `cargo nextest run -p wcore-tools --no-capture --retries 0 --no-fail-fast -E \"test(a_save_during_an_edit_is_not_lost)\"` run twelve times. `a_save_during_an_edit_is_not_lost`: 6 of 12 green; 5 of 12 RED on real loss, verbatim `[edit/rename] window 171.1952ms; 2 lost, 18 interleavings caught`, `... 827.5497ms; 1 lost, 5 ...`, `... 591.0507ms; 2 lost, 10 ...`, `... 255.845ms; 1 lost, 13 ...`, `... 159.1729ms; 1 lost, 17 ...`, i.e. 7 lost of 169 interleavings (4.1%) over the eleven executions that measured anything; 1 of 12 never measured because the FIXTURE's own saver failed at inv2_round5_adversarial_test.rs:565 with `Os { code: 5, kind: PermissionDenied, message: \"Access is denied.\" }`. `a_save_during_an_edit_is_not_lost_on_the_vfs_path`: 7 green, 1 red on loss (`[edit/vfs/rename] window 211.5819ms; 1 lost, 22 interleavings caught`, 1 of 144 aggregated, 0.7%), 3 red on the same rename refusal, 1 timed out. THE AMBIGUITY THE CRITERION NAMES, RESOLVED - AND NOT THE WAY THE CRITERION GUESSED. It offered two explanations for the arms being green on the Windows CI leg, `the race is rare enough to be luck` or `retries are absorbing it`. RETRIES ARE RULED OUT, checked rather than assumed: `.config/nextest.toml:627-628` pins `binary(=inv2_round5_adversarial_test)` to `retries = 0` under `profile.ci`, so the Windows CI leg already runs these arms with no retry at all. The difference between a Windows CI leg that has been green and a Windows workstation that is 11-of-24 red at the same retry setting is therefore the HOST, not the retry policy, and that is left OPEN rather than answered here - it is named in FerroxLabs/wayland-core#370. LOAD IS PART OF IT AND IS DECLARED: the six slow executions (40-97s wall, cold tree) carried four of the five loss failures and the six fast ones (15-18s, warm) carried one, so these figures are an upper bound for a warm host. Loss is not purely a load artefact though - the 1-of-17 came from a 17.0s warm execution and one rename refusal from a 14.9s one. Same cold-tree sensitivity the workspace-size latency work recorded, and the reason a clean small temp dir hides all of it. Both arms now carry a `# The Windows truth, measured` doc section with these figures and with the second failure shape (the editor's own save refused by the OS) named. They are deliberately left UNGATED so they keep reporting the defect; the repair is FerroxLabs/wayland-core#370 (c3). Control on the primitive itself: `atomic_io::tests::the_check_is_handed_the_bytes_the_publish_displaced`, the instrument the previous note said had never been observed to run, was executed on the same host - `1 test run: 1 passed` - so `ReplaceFileW` does work here and the losses are the fallback window, not a primitive that never fires."
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

## Re-graded at HEAD by lane f13-u-flake-chan, 2026-08-29

OWNED ELSEWHERE, NOT DUPLICATED. The Windows half is worked on
`lane/f13-fin-windows-runs` (`bd1845638` grades ten Windows criteria on
SEANDESKTOP, `5ddea3e7e` corrects the retries claim on c5). On that lane c5 is
`met` from a measured Windows rate and c3 is `superseded` with a handoff to
`FerroxLabs/wayland-core#370`. Neither commit is in `origin/integ/f13`, so the
rows above still read `not-met` on THIS tree.

Two of that lane's load-bearing claims were re-checked here against the tree
rather than accepted:

* `retries are ruled out` -- TRUE at this commit. `.config/nextest.toml:626-628`
  carries `[[profile.ci.overrides]] filter = 'binary(=inv2_round5_adversarial_test)'`
  with `retries = 0`, so the Windows CI leg already runs these arms with no
  retry. A Windows CI leg that is green while the same tree is 11-of-24 red on a
  Windows workstation at the same retry setting is therefore a HOST difference,
  which is what #370 carries.
* `every ReplaceFileW failure degrades silently` -- TRUE at this commit.
  `crates/wcore-config/src/atomic_io.rs` ends its Windows `publish_displacing`
  with `Ok(Swap::Unsupported)` after binding `err` and using it only to test for
  `ERROR_FILE_NOT_FOUND`; the sharing violation an open editor produces is not
  distinguished, and there is no log, counter or returned reason on that path.

The Linux/macOS half (c1, c2, c4) was re-checked at HEAD by symbol:
`atomic_io::atomic_write_checked`, `unsaved_work::pre_image_matches` (:1504) and
`changed_under_write` (:1518) are present, and the three arms named by the rows
(`inv2_round5_adversarial_test.rs:630`, `:680`, `:1051`) all exist.

No work taken here: the remainder is #370's contract, which is filed with its own
gradeable acceptance, and duplicating it would race that lane.
