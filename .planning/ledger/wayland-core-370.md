---
issue: 370
repo: FerroxLabs/wayland-core
kind: defect
title: "Edit-vs-save loses data on Windows: 7 of 169 interleavings lost at retries=0, and every ReplaceFileW failure degrades silently to the racy fallback"
status: open
last_verified_commit: 4895dd38
criteria:
  - id: c1
    text: "The two named arms pass at retries=0 over N >= 20 on Windows, OR they are gated with the measured Windows rate recorded and a separate arm grading whatever weaker guarantee Windows is declared to give"
    state: met
    evidence: "test:crates/wcore-config/src/atomic_io.rs::a_refused_replacefilew_is_counted_and_not_silent"
    owner: core
    note: "MET 2026-08-30 on the SECOND branch, lane w3-windows-honesty, and the first branch was REFUTED by measurement rather than assumed away. THE DECIDING CONDITION WAS RE-RUN, not inherited: this criterion could only be met by gating if the arms still fail, and FerroxLabs/wayland#1202 changed Swap semantics on the very path they measure and is already merged, so the pre-#1202 numbers could not settle it. Re-measured 2026-08-30 on SeanDesktop (Windows 10.0.26200.9168) against THIS branch, N=20 per arm, `--run-ignored all --retries 0 --no-fail-fast`, so the forced-on ignore does not hide the thing being asked about: `a_save_during_an_edit_is_not_lost` red in 6 of 20, losing 3 of 302 interleaved saves (1.0%) across the 16 executions that measured a window; `a_save_during_an_edit_is_not_lost_on_the_vfs_path` red in 8 of 20, losing 1 of 219 (0.5%) across 13; the other 11 reds printed no window because the fixture rename was refused with ERROR_ACCESS_DENIED, which is #370's SECOND Windows failure and not an absence of one. 14 of 40 executions red. So the arms do NOT pass at retries=0 over N>=20 and gating is the honest branch. Both arms carry `#[cfg_attr(windows, ignore = ...)]` whose reason string records BOTH measurements (the original and this re-run) rather than deleting the older one, and the separate arm grading the weaker guarantee is `wcore_config::atomic_io::tests::a_refused_replacefilew_is_counted_and_not_silent` (c2). NOT CLOSED BY THIS: no save-loss is fixed. What is closed is that Windows no longer claims the unix guarantee, and the arms no longer pass only because the nextest profile retries."
  - id: c2
    text: "A negative control proves the silent-degrade path is observable when it fires"
    state: met
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::degraded_publish_count"
    owner: core
    note: "MET 2026-08-30, lane w3-windows-honesty, and it is a REPRODUCTION rather than a model. `Swap::Unsupported` now carries the OS reason verbatim instead of collapsing every cause into one variant, `note_degraded_publish` counts the degrade in `DEGRADED_PUBLISHES` and logs it at `error!` -- error! and not warn!, because with RUST_LOG unset only ERROR reaches stderr and a warning would be exactly as invisible as the silence it replaces -- and `degraded_publish_count()` exposes it. The control opens the destination with `CreateFileW(FILE_GENERIC_READ, FILE_SHARE_READ|FILE_SHARE_WRITE)` -- an editor's handle, shared for read and write but NOT for delete -- so `ReplaceFileW` must rename that destination aside, needs DELETE access, and is refused with ERROR_SHARING_VIOLATION. That is the exact path #370 measured losing bytes, not a stand-in for it. The test asserts only that the degrade was COUNTED and deliberately does not assert the write's outcome, because with the handle held the fallback's own persist is refused too, which is #370's second failure mode; asserting an outcome there would make the arm measure the wrong thing. `>` and not `+1` because the counter is process-global and siblings run in parallel. GREEN ON REAL WINDOWS 10.0.26200.9168, 2026-08-30, exit 0. RED ARM: `note_degraded_publish` reduced to a no-op reddens it and nothing else -- see the lane report."
---

GRADED 2026-08-30 by lane w3-windows-honesty. Both criteria are met, and the
issue's own framing is what makes that honest rather than convenient: c1 offered
a choice between fixing the loss and DECLARING the weaker guarantee, and the
declaring branch is taken.

READ THIS BEFORE CLOSING. Nothing here fixes a lost save. On Windows a save
that arrives inside the check window can still be lost, at a rate re-measured
2026-08-30 as 3 of 302 on the Edit path and 1 of 219 on the VFS path, and the
editor's own rename is still refused outright in about a quarter of executions.
What changed is that the product now says so -- in `atomic_io`'s own words, in
both arms' ignore reasons, and at `error!` with a counter every time a publish
degrades -- instead of passing CI because the nextest profile retries.
