---
issue: 1155
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: an Edit can overwrite a save that arrives while the guard is checking it (TOCTOU), and retries=2 hides it"
status: open
last_verified_commit: cb2bf1a4
criteria:
  - id: c1
    text: "A guarded write publishes through an atomic compare-and-exchange rather than a re-check"
    state: met
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::Swap"
    owner: core
    note: "measured on the path the dispatcher actually takes, interleaved at n=200 per arm: 160 losses before, 0 after. The pre-existing suite test went 14/48 -> 0/48"
  - id: c2
    text: "The same guarantee holds on Windows"
    state: met
    evidence: "test:crates/wcore-config/src/atomic_io.rs::the_check_is_handed_the_bytes_the_publish_displaced"
    owner: core
    note: "publish_displacing now uses ReplaceFileW with lpBackupFileName on Windows (atomic_io.rs:327-380), so the check reads the bytes the publish displaced on every platform and the assertion is no longer cfg-split. TWO caveats: ReplaceFileW is not an exchange (there is an instant at which the destination name does not resolve), so c1's literal compare-and-exchange wording holds on Unix only; and this arm is UNEXECUTED by the lane that wrote it - it ships on cargo check --target x86_64-pc-windows-gnu plus the Windows CI job."
  - id: c3
    text: "No remaining test tolerates data loss as a pass"
    state: met
    evidence: "test:crates/wcore-tools/tests/inv2_round5_adversarial_test.rs::an_in_place_save_is_not_lost_to_the_final_rename"
    owner: core
    note: "Renamed from an_in_place_save_can_still_lose_to_the_final_rename; the tolerant bound lost*4 < interleaved is replaced by assert_eq!(lost, 0). Re-graded 12 runs x 24 attempts on hetzner: 230/288 interleaved, 0 lost. Every lost assertion in the file (531, 542, 634, 683) now asserts zero."
  - id: c4
    text: "A regression of this race cannot be retried into a green board: the adversarial test runs at retries=0"
    state: met
    evidence: "file:.config/nextest.toml:627"
    owner: core
    note: "CLOSED. `[[profile.ci.overrides]] filter = 'binary(=inv2_round5_adversarial_test)' / retries = 0` is added at .config/nextest.toml:626-628, scoped to the binary the way the three blocks above it are scoped, so every other test keeps retries = 2. PROVEN, not asserted, on hetzner-dsm 2026-08-29 with a deterministic probe temporarily added to THAT binary -- it fails on its first attempt and passes on any retry, which is the exact shape a race takes and the exact shape nextest launders. Only the override differed between the two arms. Arm A, `cargo nextest run --profile ci` with the override absent: `TRY 1 FAIL [   0.005s] (---) wcore-tools::inv2_round5_adversarial_test wl1155_retry_laundering_probe` / `TRY 2 PASS [   0.005s] (1/1)` / `Summary [   0.019s] 1 test run: 1 passed (1 flaky), 1812 skipped` / `CARGO_EXIT=0` -- laundered green. Arm B, same command with the override present: `FAIL [   0.005s] (1/1) wcore-tools::inv2_round5_adversarial_test wl1155_retry_laundering_probe` / `Summary [   0.013s] 1 test run: 0 passed, 1 failed, 1812 skipped` / `CARGO_EXIT=100`. The probe was removed; the whole binary then runs 17/17 PASS under the override. THE REAL RACE ALSO REDDENS: with publish_displacing forced to Swap::Unsupported and the check window held open, `thread 'an_in_place_save_is_not_lost_to_the_final_rename' (670051) panicked at crates/wcore-tools/tests/inv2_round5_adversarial_test.rs:687:5: / assertion left == right failed: an in-place save that arrived while the write was being checked was overwritten: 4 of 12 interleavings lost`. WORTH RECORDING AGAINST FUTURE GRADING: withdrawing the exchange ALONE did not redden the test on this host -- the fallback's own window is sub-millisecond here and 8 of 8 runs at a 1ms held window failed while 1 of 1 at 0ms passed. So a reintroduced race is a NARROW-window failure, which is precisely the intermittent kind retries=2 launders, and precisely why this override is the fix rather than a formality."
  - id: c5
    text: "A rollback that exchanged nothing is never reported to the caller as a clean refusal"
    state: met
    evidence: "test:crates/wcore-config/src/atomic_io.rs::a_rollback_that_exchanged_nothing_is_not_a_clean_refusal"
    owner: core
    note: "Found while verifying c1/c2 and fixed here. `restore` was `publish_displacing(displaced, dest).map(|_| ())`, which discarded the `Swap` discriminant: `Vacant` (the destination name disappeared between the publish and the verdict -- an external rm, a git checkout, an editor that unlinks before writing) and `Unsupported` both answered Ok having exchanged NOTHING. Control then fell to discard_displaced, which unlinks the pre-image -- the only surviving copy -- and atomic_write_checked returned Ok(Err(why)), whose documented contract is that the destination is exactly as it was; edit.rs:536 / write.rs:454 then rendered changed_under_write over published data loss. REPRODUCED on the unmodified tree by unlinking the destination inside the accept callback: `a rollback that exchanged nothing was reported as Err(\"changed under write\") -- the caller will tell the user the destination is untouched. Surviving files in the directory: []`. Total loss. FIXED at atomic_io.rs by matching the Swap so both non-exchange arms are errors and the existing keep_displaced preservation path runs; the caller now gets a hard Err naming where the bytes were kept, and the test reads them back. RED ARM re-run after the fix: restoring the `.map(|_| ())` one-liner reddens the test (10 run: 9 passed, 1 failed) and restoring the match returns 10/10. SCOPE: exchange platforms only -- the Windows arm of `restore` is a replacing fs::rename, which reports the failure it had, so the class has one instance and it is closed."
---

The reported race is fixed and measured. Two residuals keep it open, and the
second one is the reason this ledger exists.

c3 is a test that PASSES while permitting exactly the loss the ticket
reports. It was written before the fix, never re-graded after it, and would
have gone unmentioned in any narrative handoff — the suite is green, so the
prose says green.

Also tracked as #342 on the wayland-core tracker, which describes the same
defect from the other side. Grade them together.
