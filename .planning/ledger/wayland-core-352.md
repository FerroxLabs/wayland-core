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
    note: "CORRECTION 2026-08-29 (lane f13-fin-hetzner-residuals): THIS CRITERION WAS FALSE ON integ/f13 WHEN IT WAS GRADED MET, and the cause was not the sweep. The throwaway macOS red-arm instrument from lane/f13-352-macos-redarm -- the one c5 quotes and calls `a THROWAWAY instrument branch`, whose own comment in the code reads `never merged` -- WAS merged into integ/f13. It sat at the top of the cfg(unix) OwnedTree::snapshot as `if std::hint::black_box(true) { return; }`, so `known` stayed empty, `kill_all` killed nothing, and every one of the swept sites owned only the LEAF on Linux AND macOS at once: exactly the #1156 defect the sweep exists to close, reintroduced under the sweep rather than through it. It was invisible to the sweep and the ratchet by construction, because both grade the CALL SITES and this neutered the GUARD. Caught by running the lane gate, not by reading: dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child failed 3/3 in the suite and 2/2 alone on this lane, and IDENTICALLY 2/2 on a detached worktree of untouched origin/integ/f13 -- which is what ruled the lane out and pinned it on the integration tree. Verbatim, on the untouched base: `thread \'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child\' (1655037) panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: / the grandchild 1655039 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)`. The instrument is deleted here and the real body restored; the same test now PASSES in 0.214s where it had been timing out at 10.1s. The tree was also swept for other merged instruments -- the remaining black_box occurrences are a symbol-liveness pin in packaged_runtime.rs and an unrelated `Harness::BlackBox` enum, neither a mutation. MERGED into integ/next as 2165c30a on 2026-08-29. The sweep covers every .spawn() and .spawn_command() site under crates/wcore-cli/tests; ALLOWED_UNOWNED is EMPTY, which is the ticket's 'no unstated remainder', and MINIMUM_KNOWN_SITES = 40 floors the scan so a reader that has stopped seeing the tree cannot pass by seeing nothing. 04681f33 added the last one (the PTY probe in quarantine_terminal_authority)."
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
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-fin-hetzner-residuals). The Linux and macOS arms were already quoted verbatim in the note this replaces and are UNCHANGED -- see the previous revision of this entry for both, including macOS run https://github.com/FerroxLabs/wayland-core/actions/runs/33244213309 job 99078722412 at 8d6add71, where exactly one test failed in the whole leg and it reddened at the ownership assertion rather than the precondition, proving the pgrep descendant walk RAN on macOS. THE WINDOWS ARM IS NOW OBSERVED, which is what this criterion was one arm short of. It was never unattempted, only unreddenable: it needed #358 to land a real Windows mechanism first, and #358 c1 IS in integ/f13. HOST: SeanDesktop, the only Windows machine, D:\\resid358 at lane commit d35ac0a0, cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows. GREEN FIRST, so the red is a difference and not a broken test: `PASS [ 0.290s] (5/5) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows` / `Summary [ 0.291s] 5 tests run: 5 passed, 0 skipped`. RED ARM, verbatim, with the job withdrawn (both TerminateJobObject sites and the Drop CloseHandle put behind `std::hint::black_box(false)` in wcore-types/src/job_object.rs -- the same black_box shape the macOS arm used, chosen so the rest of the body stays reachable; the file was touched after the mutation so cargo could not measure the old binary): `thread \'dropping_the_guard_kills_a_detached_grandchild_on_windows\' (33376) panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:109:5: / the grandchild 38828 outlived the guard -- on Windows killing the direct child does not reach a descendant, so without a Job Object the guard owns the leaf and leaks the TREE (FerroxLabs/wayland-core#358)` / `Summary [ 20.530s] 5 tests run: 4 passed, 1 failed, 0 skipped` / `EXIT=100`. WHY THAT MUTATION AND NOT A REVERT OF owned_tree.rs: this entry\'s previous note recorded that withdrawing the descendant walk outright reddens at the PRECONDITION instead -- the test then refuses to grade tree ownership at all rather than failing the ownership assertion, which is a red arm about the instrument, not about the guard. Withdrawing only the KILLING keeps the job created and assigned, so the kernel still answers `the grandchild is in this job` (the anti-vacuity check at :74) and the direct child still dies (:105 passed, since execution reached :109) -- leaving exactly the pre-#358 shape, the guard owning the LEAF and leaking the TREE. The mutation was reverted with git checkout, the file touched, and the green re-measured after the revert."

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
