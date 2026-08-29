---
issue: 385
repo: FerroxLabs/wayland-core
kind: defect
title: "every_spawn_site_owns_its_tree grades WRAPPING, never ownership, so it stayed green through a period the guard owned nothing"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The ratchet cannot pass while OwnedTree's descendant walk is a stub -- either it asserts a behavioural property, or #352/#1156 stop citing it as class closure and the criterion text says it is a wrapping ratchet only"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D20, found while verifying FerroxLabs/wayland#1156). Nothing has been done. The measured finding, verbatim: The #352 ratchet passes while the guard it ratchets owns nothing. `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` scans source text for `.spawn()` / `.spawn_command(` expressions not handed to `OwnedTree` — it grades WRAPPING, never OWNERSHIP — so it stayed green through the entire period the descendant walk was dead. The only thing that caught the neutering was the single behavioural test `harness_owns_spawned_trees`."
  - id: c2
    text: "A red arm is quoted: with the descendant walk neutered, the gate cited as class closure goes RED"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D20). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "harness_owns_spawned_trees cannot be skipped, quarantined or made platform-conditional without the ratchet going red too"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D20). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The #352 ratchet passes while the guard it ratchets owns nothing. `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` scans source text for `.spawn()` / `.spawn_command(` expressions not handed to `OwnedTree` — it grades WRAPPING, never OWNERSHIP — so it stayed green through the entire period the descendant walk was dead. The only thing that caught the neutering was the single behavioural test `harness_owns_spawned_trees`.

**Where.** crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs (whole file); observed green in the same run where harness_owns_spawned_trees was red

**Why it matters.** The ratchet reads as the class-closure gate for #1156/#352 and will be cited as one, but it cannot distinguish a working guard from a stub. If the behavioural test is ever quarantined, skipped or made platform-conditional, the ratchet alone will certify a sweep that owns nothing — and it will do so in green.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
