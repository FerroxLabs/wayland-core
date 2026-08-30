---
issue: 1155
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: an Edit can overwrite a save that arrives while the guard is checking it (TOCTOU), and retries=2 hides it"
status: open
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "A guarded write publishes through an atomic compare-and-exchange rather than a re-check"
    state: met
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::Swap"
    owner: core
    note: "measured on the path the dispatcher actually takes, interleaved at n=200 per arm: 160 losses before, 0 after. The pre-existing suite test went 14/48 -> 0/48"
  - id: c2
    text: "The same guarantee holds on Windows"
    state: superseded
    evidence: "test:crates/wcore-config/src/atomic_io.rs::the_check_is_handed_the_bytes_the_publish_displaced"
    owner: core
    handoff: "FerroxLabs/wayland-core#370"
    note: "`handoff:` ADDED 2026-08-30: the schema has a first-class field for the ticket that carries a decomposed remainder, and naming #370 only in the prose left the successor machine-unreadable. DECOMPOSED 2026-08-30; the Windows REMAINDER is FerroxLabs/wayland-core#370, which is open. NOT graded met, and the reason is the whole point: the platform run this criterion was blocked on is DONE and green, but the criterion says `the same guarantee holds on Windows` and #370 measured on the same host that it does NOT -- 7 of 169 interleavings lost at retries=0 across the two `a_save_during_an_edit_is_not_lost` arms, because EVERY `ReplaceFileW` failure (including the sharing violation an open editor produces) degrades silently to the old re-check-then-rename fallback. Marking this met off a green unit test while the guarantee itself is measured broken on the same platform is exactly the easier-adjacent-property substitution this sweep exists to catch. WHAT IS CLOSED -- the platform run. Executed on real Windows -- SeanDesktop, `Microsoft Windows [Version 10.0.26200.9168]` -- on 2026-08-30, at integ/f13 a278f8c3 (`crates\\wcore-config\\src\\atomic_io.rs` SHA256 C8077AF7A91726A7F2CCE58297C93BA8216E3F5B314B736556E4DF9E58F5B68D). ARM A, unmodified tree: `test atomic_io::tests::the_check_is_handed_the_bytes_the_publish_displaced ... ok` / `test result: ok. 1 passed; 0 failed; ... 752 filtered out`, BASE_EXITCODE=0. Not a cross-compile: the previous grade rested on `cargo check --target x86_64-pc-windows-gnu`, which compiles the assertion and runs nothing. RED ARM ON THE SAME HOST, tree committed first: `publish_displacing`'s Windows body forced to `return Ok(Swap::Unsupported)` -- ReplaceFileW withdrawn, the fallback rename path taken -- with the mutation confirmed to land on EXECUTABLE code (printed in context: atomic_io.rs:342 `let _ = (a, b);` / :343 `return Ok(Swap::Unsupported);` / :344 `let Some(stem) = a.file_name() else {`) and the file SHA256 changing to 7ED4D60168A449C5C2A7BBF6D83EB0324B1ABE120528CB94E111DC4DFC8F426C. RED_EXITCODE=101, verbatim: `assertion left == right failed: the publish precedes the check, so the check is handed what it displaced / left: [111, 108, 100] / right: [110, 101, 119]` -- the check was handed `old` where it must be handed `new`. RESTORED byte-identical (SHA256 back to C8077A..., `git diff --numstat` empty), LastWriteTime touched so cargo could not skip the rebuild, re-run RESTORED_EXITCODE=0. THE ORIGINAL TWO CAVEATS STAND and are not closed by this run: ReplaceFileW is not an exchange (there is an instant at which the destination name does not resolve), so c1's literal compare-and-exchange wording holds on Unix only; and the Windows CI test legs are still SKIPPED on these branches, so this evidence is a named host run, not a standing gate. WHAT IS NOT CLOSED is the silent degrade and the residual loss rate, which is #370's c1 and c2 verbatim and is owned there."
  - id: c3
    text: "No remaining test tolerates data loss as a pass"
    state: met
    evidence: "test:crates/wcore-tools/tests/inv2_round5_adversarial_test.rs::an_in_place_save_is_not_lost_to_the_final_rename"
    owner: core
    note: "Renamed from an_in_place_save_can_still_lose_to_the_final_rename; the tolerant bound lost*4 < interleaved is replaced by assert_eq!(lost, 0). Re-graded 12 runs x 24 attempts on hetzner: 230/288 interleaved, 0 lost. Every lost assertion in the file (531, 542, 634, 683) now asserts zero."
  - id: c4
    text: "A regression of this race cannot be retried into a green board: the adversarial test runs at retries=0"
    state: met
    evidence: "file:.config/nextest.toml:714:data-loss assertion, so none of them may be retried into a pass"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was nextest.toml:652, a line of the comment block belonging to the #1146 override above; it now cites the #1155 override itself, whose `retries = 0` is the claim. CLOSED. `[[profile.ci.overrides]] filter = 'binary(=inv2_round5_adversarial_test)' / retries = 0` is added at .config/nextest.toml:651-653, scoped to the binary the way the three blocks above it are scoped, so every other test keeps retries = 2. PROVEN, not asserted, on hetzner-dsm 2026-08-29 with a deterministic probe temporarily added to THAT binary -- it fails on its first attempt and passes on any retry, which is the exact shape a race takes and the exact shape nextest launders. Only the override differed between the two arms. Arm A, `cargo nextest run --profile ci` with the override absent: `TRY 1 FAIL [   0.005s] (---) wcore-tools::inv2_round5_adversarial_test wl1155_retry_laundering_probe` / `TRY 2 PASS [   0.005s] (1/1)` / `Summary [   0.019s] 1 test run: 1 passed (1 flaky), 1812 skipped` / `CARGO_EXIT=0` -- laundered green. Arm B, same command with the override present: `FAIL [   0.005s] (1/1) wcore-tools::inv2_round5_adversarial_test wl1155_retry_laundering_probe` / `Summary [   0.013s] 1 test run: 0 passed, 1 failed, 1812 skipped` / `CARGO_EXIT=100`. The probe was removed; the whole binary then runs 17/17 PASS under the override. THE REAL RACE ALSO REDDENS: with publish_displacing forced to Swap::Unsupported and the check window held open, `thread 'an_in_place_save_is_not_lost_to_the_final_rename' (670051) panicked at crates/wcore-tools/tests/inv2_round5_adversarial_test.rs:687:5: / assertion left == right failed: an in-place save that arrived while the write was being checked was overwritten: 4 of 12 interleavings lost`. WORTH RECORDING AGAINST FUTURE GRADING: withdrawing the exchange ALONE did not redden the test on this host -- the fallback's own window is sub-millisecond here and 8 of 8 runs at a 1ms held window failed while 1 of 1 at 0ms passed. So a reintroduced race is a NARROW-window failure, which is precisely the intermittent kind retries=2 launders, and precisely why this override is the fix rather than a formality."
---

The reported race is fixed and measured. Two residuals keep it open, and the
second one is the reason this ledger exists.

c3 is a test that PASSES while permitting exactly the loss the ticket
reports. It was written before the fix, never re-graded after it, and would
have gone unmentioned in any narrative handoff — the suite is green, so the
prose says green.

Also tracked as #342 on the wayland-core tracker, which describes the same
defect from the other side. Grade them together.
