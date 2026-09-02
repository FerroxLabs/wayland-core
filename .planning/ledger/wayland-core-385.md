---
issue: 385
repo: FerroxLabs/wayland-core
kind: defect
title: "every_spawn_site_owns_its_tree grades WRAPPING, never ownership, so it stayed green through a period the guard owned nothing"
status: closed
last_verified_commit: 93ede3424
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
    evidence: "file:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs:361:does not include its own grandchild"
    owner: core
    note: "MET AS WRITTEN. RED ARM, on executable code, function body printed before and after: `descendants()` in crates/wcore-cli/tests/support/owned_tree.rs:127 was stubbed with `if root > 0 { return Vec::new(); }` -- the exact 'walk reports no descendants for a tree that has them' shape the module docs warn about. `cargo nextest run -p wcore-cli --test every_spawn_site_owns_its_tree` then exits 100 with: "`OwnedTree`'s descendant walk reported [] for pid 773411, which does not include its own grandchild 773413. The walk is a stub or has stopped reading /proc, so every guard in this crate owns a LEAF and leaks the TREE -- and the wrapping scan above would still be green". Restored with `git checkout --` and `touch` (an untouched restore leaves an older mtime and cargo measures the mutated binary); the restored run is 25 passed, 0 failed."
  - id: c3
    text: "harness_owns_spawned_trees cannot be skipped, quarantined or made platform-conditional without the ratchet going red too"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::assert_the_behavioural_twins_are_armed"
    owner: core
    note: "MET, but by an ALLOWLIST and not by the enumeration this note used to claim. An independent verifier refuted the old argument: the guard refused two literals, `#[ignore` and `#[cfg(`, and `#[cfg_attr(cond, ignore)]` matches NEITHER while being simultaneously a skip and a platform-condition -- both nouns in the criterion. It is also this repo's house idiom for a platform skip: 25 live cfg_attr-plus-ignore sites, one of whose module doc teaches it. A SECOND hole of the same shape was found while fixing the first -- a file-level `#![cfg_attr(cond, cfg(any()))]` is not `#![cfg(`, so it evaded the inner-gate count and compiled the entire twin out; the pre-fix guard scored exit 0 on it, 25 tests run, 0 skipped, with the twin binary contributing zero tests. RE-GRADED BY CONSTRUCTION rather than by adding cfg_attr to a list: whitespace is stripped, then the twin's file must carry EXACTLY ONE inner attribute and it must be the gate, and the twin's own attribute block must be EXACTLY `#[test]`. Is-a-skip is undecidable over an open alphabet of attribute macros; is-not-#[test] is decidable and total, so every present and future skipping attribute reds it. RED ARMS, all on executable code, each with `cargo check -p wcore-cli --tests` exit 0 BEFORE the run and each restored with `git checkout` plus `touch` and the restored blob compared to origin: (1) `#[cfg_attr(not(target_os = macos), ignore = ...)]` on the unix twin -> exit 100, where the pre-fix guard was exit 0 with that twin reported skipped; (2) bare `#[ignore]` -> exit 100, the original arm kept as positive control; (3) file-level `#![cfg_attr(not(target_os = macos), cfg(any()))]` -> exit 100; (4) `#[cfg(target_os = macos)]` on the fn -> exit 100; (5) `#[cfg_attr(cond, ignore)]` on the WINDOWS twin, graded from Linux because the check is over source text -> exit 100. NEGATIVE CONTROL: the unmutated tree is exit 0, 49 tests run, 0 skipped, so the guard is not simply always-red, and the attribute reader carries its own two-polarity synthetic control in the same call. NAMED GAPS, stated rather than hidden -- the allowlist is closed over ATTRIBUTES only and does not see a body-internal early return or `cfg!` guard (the twin then runs vacuously, which is why `assert_the_guard_actually_owns_the_tree` drives the kernel check from the ratchet's own binary), a nextest default-filter that excludes the twin's binary without naming the test, or a CI job that never invokes that binary. Quarantine, deletion and rename are unchanged: the two by-name lists, and the read-or-panic on each twin path."
---

The #352 ratchet passes while the guard it ratchets owns nothing. `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` scans source text for `.spawn()` / `.spawn_command(` expressions not handed to `OwnedTree` — it grades WRAPPING, never OWNERSHIP — so it stayed green through the entire period the descendant walk was dead. The only thing that caught the neutering was the single behavioural test `harness_owns_spawned_trees`.

**Where.** crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs (whole file); observed green in the same run where harness_owns_spawned_trees was red

**Why it matters.** The ratchet reads as the class-closure gate for #1156/#352 and will be cited as one, but it cannot distinguish a working guard from a stub. If the behavioural test is ever quarantined, skipped or made platform-conditional, the ratchet alone will certify a sweep that owns nothing — and it will do so in green.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
