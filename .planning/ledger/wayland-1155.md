---
issue: 1155
repo: FerroxLabs/wayland
title: "[Bug]: an Edit can overwrite a save that arrives while the guard is checking it (TOCTOU), and retries=2 hides it"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A guarded write publishes through an atomic compare-and-exchange rather than a re-check"
    state: met
    evidence: "symbol:crates/wcore-config/src/atomic_io.rs::Swap"
    owner: core
    note: "measured on the path the dispatcher actually takes, interleaved at n=200 per arm: 160 losses before, 0 after. The pre-existing suite test went 14/48 -> 0/48"
  - id: c2
    text: "The same guarantee holds on Windows"
    state: not-met
    owner: core
    note: "there is no exchange primitive there — atomic_io.rs:251-254 returns Swap::Unsupported — so the publish degrades to re-check-then-rename and the race stays open. The product ships on Windows"
  - id: c3
    text: "No remaining test tolerates data loss as a pass"
    state: not-met
    owner: core
    note: "an_in_place_save_can_still_lose_to_the_final_rename tolerates up to 25% data loss, dates to v0.13.0, and was never re-graded after the exchange landed. That is this ticket's literal symptom, still measurable on the FIXED platform"
---

The reported race is fixed and measured. Two residuals keep it open, and the
second one is the reason this ledger exists.

c3 is a test that PASSES while permitting exactly the loss the ticket
reports. It was written before the fix, never re-graded after it, and would
have gone unmentioned in any narrative handoff — the suite is green, so the
prose says green.

Also tracked as #342 on the wayland-core tracker, which describes the same
defect from the other side. Grade them together.
