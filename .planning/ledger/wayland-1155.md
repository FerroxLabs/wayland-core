---
issue: 1155
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: an Edit can overwrite a save that arrives while the guard is checking it (TOCTOU), and retries=2 hides it"
status: open
last_verified_commit: 43848f75
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
    state: not-met
    owner: core
    note: "The 'retries=2 hides it' half of the ticket title had no criterion. .config/nextest.toml still sets [profile.ci] retries = 2 and inv2_round5_adversarial_test appears in NONE of the retries=0 override filters, so the exact race that survived unnoticed would survive unnoticed again."
---

The reported race is fixed and measured. Two residuals keep it open, and the
second one is the reason this ledger exists.

c3 is a test that PASSES while permitting exactly the loss the ticket
reports. It was written before the fix, never re-graded after it, and would
have gone unmentioned in any narrative handoff — the suite is green, so the
prose says green.

Also tracked as #342 on the wayland-core tracker, which describes the same
defect from the other side. Grade them together.
