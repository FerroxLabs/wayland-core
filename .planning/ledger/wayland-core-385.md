---
issue: 385
repo: FerroxLabs/wayland-core
kind: defect
title: "every_spawn_site_owns_its_tree grades WRAPPING, never ownership, so it stayed green through a period the guard owned nothing"
status: open
last_verified_commit: 70a47aaed
criteria:
  - id: c1
    text: "The ratchet cannot pass while OwnedTree's descendant walk is a stub -- either it asserts a behavioural property, or #352/#1156 stop citing it as class closure and the criterion text says it is a wrapping ratchet only"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::assert_the_guard_actually_owns_the_tree"
    owner: core
    note: "MET AS WRITTEN by the FIRST branch: the ratchet now asserts a behavioural property, so #352/#1156 may keep citing it. `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` still grades WRAPPING from source text -- that part is in the source or nowhere -- but it now ends by asking the KERNEL. `assert_the_guard_actually_owns_the_tree` drives the same fixture the behavioural twins use, checks the direct child AND the grandchild are actually alive first (an unlaunched fixture would let everything below pass for the wrong reason), then requires OwnedTree's /proc walk to SEE the grandchild on Unix / the Job Object to CONTAIN it on Windows, and finally requires dropping the guard to kill it. Ownership is the kill, not the sighting."
  - id: c2
    text: "A red arm is quoted: with the descendant walk neutered, the gate cited as class closure goes RED"
    state: met
    evidence: "file:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs:260:does not include its own grandchild"
    owner: core
    note: "MET AS WRITTEN. RED ARM, on executable code, function body printed before and after: `descendants()` in crates/wcore-cli/tests/support/owned_tree.rs:127 was stubbed with `if root > 0 { return Vec::new(); }` -- the exact 'walk reports no descendants for a tree that has them' shape the module docs warn about. `cargo nextest run -p wcore-cli --test every_spawn_site_owns_its_tree` then exits 100 with: "`OwnedTree`'s descendant walk reported [] for pid 773411, which does not include its own grandchild 773413. The walk is a stub or has stopped reading /proc, so every guard in this crate owns a LEAF and leaks the TREE -- and the wrapping scan above would still be green". Restored with `git checkout --` and `touch` (an untouched restore leaves an older mtime and cargo measures the mutated binary); the restored run is 25 passed, 0 failed."
  - id: c3
    text: "harness_owns_spawned_trees cannot be skipped, quarantined or made platform-conditional without the ratchet going red too"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::assert_the_behavioural_twins_are_armed"
    owner: core
    note: "MET AS WRITTEN, all THREE named ways, each proven by a DIFFERENT mutation and each restored with `git checkout --` + `touch`. (1) SKIPPED: `#[ignore]` on `dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child` -> exit 100, 'harness_owns_spawned_trees.rs carries #[ignore], so the only behavioural proof of tree ownership no longer runs'. (2) QUARANTINED: adding the test name to .config/known-failing-tests.txt -> exit 100, '.config/known-failing-tests.txt names `dropping_the_guard...`, which silences or excuses the only behavioural proof'. (3) PLATFORM-CONDITIONAL: `#[cfg(target_os = "linux")]` on the test -> exit 100, 'carries its own #[cfg(...) gate on top of the file gate, so it can be compiled out on a platform where the file still builds'. Deletion and rename are covered by the read-or-panic on each twin path. Both twins are held (unix + windows), each allowed EXACTLY ONE inner cfg gate so a second one cannot narrow the pair to a platform subset, and the quarantine-list predicate carries its own positive AND negative control in the same call -- it must find a line that IS there and must NOT match a comment -- because an empty result reads exactly like 'not quarantined'."
---

The #352 ratchet passes while the guard it ratchets owns nothing. `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` scans source text for `.spawn()` / `.spawn_command(` expressions not handed to `OwnedTree` — it grades WRAPPING, never OWNERSHIP — so it stayed green through the entire period the descendant walk was dead. The only thing that caught the neutering was the single behavioural test `harness_owns_spawned_trees`.

**Where.** crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs (whole file); observed green in the same run where harness_owns_spawned_trees was red

**Why it matters.** The ratchet reads as the class-closure gate for #1156/#352 and will be cited as one, but it cannot distinguish a working guard from a stub. If the behavioural test is ever quarantined, skipped or made platform-conditional, the ratchet alone will certify a sweep that owns nothing — and it will do so in green.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
