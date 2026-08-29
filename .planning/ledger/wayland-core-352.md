---
issue: 352
repo: FerroxLabs/wayland-core
title: "Test process-tree ownership: the ~40 remaining json-stream spawn sites, and the two platforms the new guard never exercises"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Every test that spawns a wayland-core child owns its tree via the shared guard, with the swept site count stated"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard"
    owner: core
    note: "MERGED into integ/next as 2165c30a on 2026-08-29. The sweep covers every .spawn() and .spawn_command() site under crates/wcore-cli/tests; ALLOWED_UNOWNED is EMPTY, which is the ticket's 'no unstated remainder', and MINIMUM_KNOWN_SITES = 40 floors the scan so a reader that has stopped seeing the tree cannot pass by seeing nothing. 04681f33 added the last one (the PTY probe in quarantine_terminal_authority)."
  - id: c2
    text: "A test fails if a new ungoverned spawn site is added, so the sweep cannot rot"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::the_ratchet_detects_an_ungoverned_site_and_accepts_a_governed_one"
    owner: core
    note: "The ratchet reads the crate's own test sources and refuses any spawn expression not handed to OwnedTree. blank_noncode blanks comments and string literals first, so the spellings quoted in the module's own prose are invisible to it - the mutation-hits-a-comment trap. Its own both-directions case is the cited test."
  - id: c3
    text: "Windows: the guard owns the tree, not just the leaf, and a test grades the grandchild case"
    state: superseded
    owner: core
    note: "Deliberately split out to wayland-core#358, which is open and carries the full Windows contract. child_pids under cfg(windows) still returns Vec::new() (owned_tree.rs:97-100) even after the sweep merged, so reap() snapshots an empty descendant set on all 40-plus swept sites at once."
  - id: c4
    text: "macOS: the pgrep arm is EXECUTED in CI at least once with the run cited, or deleted as unreachable"
    state: not-met
    owner: core
    note: "Still open after the merge. 8f7e6655 was pushed with a [ci-darwin] marker and the commit body says the marker is deliberate, but NO run URL is cited anywhere in the tree, which is what the criterion asks for. The only macOS-arm run I can find is lane/352-macos-redarm run 33235055214, which FAILED and whose branch has since been deleted; the lane/session-tickets CI run 33238005604 was still in progress at grading time. Compiling is not running, and neither is 'a run happened somewhere'."
  - id: c5
    text: "A red arm is quoted verbatim for each platform arm"
    state: not-met
    owner: core
    note: "No verbatim red arm is recorded in the tree for either platform arm. The Linux red arm exists in harness_owns_spawned_trees.rs from the #1156 work, which is a different platform; the macOS arm's only attempted red run failed for unrelated reasons and the Windows arm has no mechanism to redden yet (#358)."
---

The remainder #1156 did not ask for: own the spawned process TREE at every
wcore-cli test spawn site, not just at the five `acp serve` supervision sites,
and make the two platforms where the shared guard is not actually exercised
(Windows, macOS) either work or say plainly that they do not.

Graded against `origin/integ/next` at `43848f75`. `lane/session-tickets` merged
in as `2165c30a` while this ledger pass was running, so the sweep and the ratchet
ARE in the integration tree: every spawn site in the crate's test tree is owned,
`ALLOWED_UNOWNED` is empty, and a new ungoverned site fails the ratchet.

What is still open is the evidence, not the code. The macOS arm was pushed with
`[ci-darwin]` but no run URL is cited in-tree and the only macOS-arm run that can
be found FAILED on a branch since deleted; no verbatim red arm is recorded for
either non-Linux arm. The Windows half was correctly split out to `#358` rather
than half-shipped, and `child_pids` there still returns an empty vector.
