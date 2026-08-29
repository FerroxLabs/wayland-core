---
issue: 352
repo: FerroxLabs/wayland-core
kind: defect
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
    note: "NOT MET, and precisely one arm short. The Windows arm has no red arm and cannot have one until #358 lands, which is the whole of the remaining gap. The other two arms now have a verbatim red arm and the macOS one is cited by run URL, which is what the previous pass could not find. macOS -- CI (macos-latest) in https://github.com/FerroxLabs/wayland-core/actions/runs/33244213309 (job 99078722412), branch lane/f13-352-macos-redarm at 8d6add71, a THROWAWAY instrument branch that is deliberately NOT deleted so the evidence stays chaseable. That branch reduces OwnedTree::snapshot to leaf-only ownership behind std::hint::black_box(true), which keeps the rest of the body reachable so clippy -D warnings still passes -- the previous macOS attempt (run 33235055214) died on an unrelated lint before it reached a test, and the fmt and clippy steps of this one are green. Verbatim: `thread 'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child' (408307) panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: / the grandchild 2235 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)` / `TRY 1 FAIL [  10.204s]`, `TRY 2 FAIL [  10.168s]`, `TRY 3 FAIL [  10.085s]` / `Summary [1079.158s] 16950 tests run: 16949 passed (3 leaky), 1 failed, 118 skipped` -- EXACTLY ONE test failed in the whole macOS leg, so the red is attributable to the mutation and to nothing else. NOTE WHAT ELSE THAT RUN PROVES: it reddens at line 121, not at the line 89 precondition `child_pids(direct).contains(&grandchild)`, so the pgrep -P descendant walk RAN on macOS and FOUND the grandchild. The macOS arm is executed, not merely compiled. Linux -- two arms run on hetzner-dsm 2026-08-29 for comparison. Withdrawing the /proc walk (child_pids -> Vec::new()) reddens at the PRECONDITION instead: `panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:89:5: / the grandchild 1076605 must be visible as a descendant of 1076604; saw []`. That is the shape Windows is in TODAY, and it is the reason c5's Windows half cannot be closed here: with child_pids returning an empty vector this test refuses to grade tree ownership at all rather than failing the ownership assertion, so there is no red arm to quote and no green one either. Withdrawing only the snapshot (the same mutation the macOS branch carries) reddens at the ownership assertion on Linux too: `panicked at .../harness_owns_spawned_trees.rs:121:5: / the grandchild 1132275 outlived the guard -- ...`. WHAT WOULD SETTLE THE WINDOWS ARM: #358 landing a real descendant walk in owned_tree.rs::child_pids under cfg(windows), then one `CI (Array)` / self-hosted Windows leg on a lane branch carrying the same black_box leaf-only mutation. Until then the Windows arm is unreddenable by construction, not unattempted."
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
