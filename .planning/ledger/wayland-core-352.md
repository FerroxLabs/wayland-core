---
issue: 352
repo: FerroxLabs/wayland-core
kind: defect
title: "Test process-tree ownership: the ~40 remaining json-stream spawn sites, and the two platforms the new guard never exercises"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Every test that spawns a wayland-core child owns its tree via the shared guard, with the swept site count stated"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard"
    owner: core
    note: "CORRECTION 2026-08-29 (lane f13-fin-hetzner-residuals): THIS CRITERION WAS FALSE ON integ/f13 WHEN IT WAS GRADED MET, and the cause was not the sweep. The throwaway macOS red-arm instrument from lane/f13-352-macos-redarm -- the one c5 quotes and calls `a THROWAWAY instrument branch`, whose own comment in the code reads `never merged` -- WAS merged into integ/f13. The commit is 8d6add71, whose own SUBJECT LINE is `RED ARM (throwaway, never merge): leaf-only OwnedTree on macOS [ci-darwin]`, and `git log -S` proves it reachable from origin/integ/f13 while origin/integ/f13-base has zero black_box occurrences in that file -- so it entered through the integration, not from the base. It sat at the top of the cfg(unix) OwnedTree::snapshot as `if std::hint::black_box(true) { return; }`, so `known` stayed empty, `kill_all` killed nothing, and every one of the swept sites owned only the LEAF on Linux AND macOS at once: exactly the #1156 defect the sweep exists to close, reintroduced under the sweep rather than through it. It was invisible to the sweep and the ratchet by construction, because both grade the CALL SITES and this neutered the GUARD. Caught by running the lane gate, not by reading: dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child failed 3/3 in the suite and 2/2 alone on this lane, and IDENTICALLY 2/2 on a detached worktree of untouched origin/integ/f13 -- which is what ruled the lane out and pinned it on the integration tree. Verbatim, on the untouched base: `thread \'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child\' (1655037) panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: / the grandchild 1655039 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)`. The instrument is deleted here and the real body restored; the same test now PASSES in 0.214s where it had been timing out at 10.1s. The tree was also swept for other merged instruments -- the remaining black_box occurrences are a symbol-liveness pin in packaged_runtime.rs and an unrelated `Harness::BlackBox` enum, neither a mutation. MERGED into integ/next as 2165c30a on 2026-08-29. The sweep covers every .spawn() and .spawn_command() site under crates/wcore-cli/tests; ALLOWED_UNOWNED is EMPTY, which is the ticket's 'no unstated remainder', and MINIMUM_KNOWN_SITES = 40 floors the scan so a reader that has stopped seeing the tree cannot pass by seeing nothing. 04681f33 added the last one (the PTY probe in quarantine_terminal_authority)."
  - id: c2
    text: "A test fails if a new ungoverned spawn site is added, so the sweep cannot rot"
    state: met
    evidence: "test:crates/wcore-cli/tests/every_spawn_site_owns_its_tree.rs::the_ratchet_detects_an_ungoverned_site_and_accepts_a_governed_one"
    owner: core
    note: "The ratchet reads the crate's own test sources and refuses any spawn expression not handed to OwnedTree. blank_noncode blanks comments and string literals first, so the spellings quoted in the module's own prose are invisible to it - the mutation-hits-a-comment trap. Its own both-directions case is the cited test."
  - id: c3
    text: "Windows: the guard owns the tree, not just the leaf, and a test grades the grandchild case"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "RETIRED FROM superseded TO met 2026-08-31. The successor wayland-core#358 is now CLOSED (completed) with all 6 of its own criteria met. The anchor is the grandchild test this criterion asked for by name. NOTE the class is not closed: wayland-core#393 carries the quarantine-git-abort site of the same kill-the-leaf shape and is OPEN. This criterion's specific ask is met; the family is tracked there. -- Deliberately split out to wayland-core#358, which is open and carries the full Windows contract. child_pids under cfg(windows) still returns Vec::new() (owned_tree.rs:97-100) even after the sweep merged, so reap() snapshots an empty descendant set on all 40-plus swept sites at once."
  - id: c4
    text: "macOS: the pgrep arm is EXECUTED in CI at least once with the run cited, or deleted as unreachable"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees.rs::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child"
    owner: core
    note: "MET 2026-08-29. The macOS pgrep descendant walk is EXECUTED and GREEN in CI, cited: https://github.com/FerroxLabs/wayland-core/actions/runs/33257637102 (job 99114021304), branch lane/f13-352-macos-green2 at the head of this lane, KEPT and not deleted so the run stays chaseable. Verbatim: >>> PASS [   0.175s] ( 8121/17178) wcore-cli::harness_owns_spawned_trees dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child / Summary [1145.585s] 17178 tests run: 17177 passed (1 leaky), 1 failed, 120 skipped <<< That is the arm running against the unmutated guard on macos-latest -- pgrep -P, because macOS has no /proc -- and it is the citation the previous passes could not produce. GETTING IT REQUIRED FINDING A DEFECT FIRST, and that is the part worth keeping. A green run was impossible on integ/f13: the throwaway instrument 8d6add71 had been MERGED as d03a6e14, so OwnedTree::snapshot returned early behind black_box(true) and the guard owned the leaf only. Measured on the unmodified integration tree rather than inferred -- hetzner-dsm, 3 of 3 tries in the full nextest sweep and 3 of 3 again with the test run ALONE, so not scheduling and not load: >>> thread 'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child' panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: the grandchild 1658408 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156) <<< and on macOS in https://github.com/FerroxLabs/wayland-core/actions/runs/33255873933 (job 99109355513, branch lane/f13-352-macos-green, also kept), same test, same line, grandchild 83274. Bisected: green at bb850cc5^ (0df4c47d), red at ab6b602f. bf0b41f7 reverts the ten lines. NOTE WHERE THE RED FELL, because it settles this criterion twice over: line 121, the ownership assertion, NOT the line 89 precondition child_pids(direct).contains(&grandchild). The pgrep walk ran on macOS and FOUND the grandchild even while the snapshot was cut, so the arm is executed on the red run as well as on the green one. ONE PRE-EXISTING RED SHARES THE CITED LEG and is not this criterion's: wcore-protocol::quiescence_contract::the_published_corpus_is_current is the only other failure in the whole macOS leg, and it is Desktop-contract-corpus drift over seven files (events/ready.json, manifest.json, three adversarial/events/*.jsonl, two compat/events/*). Its Linux sibling is wcore-protocol::desktop_contract_corpus::checked_corpus_matches_real_serializers_byte_for_byte. Both are red at ab6b602f BEFORE any commit on this lane and green at 0df4c47d, verified by running them alone in clean worktrees at each commit, so they are an integration defect this lane inherited and did not cause."
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

**Updated 2026-08-29.** The macOS arm is now executed, green, and cited by run
URL (c4), and both non-Linux arms that can have a red arm now have one quoted
verbatim. What is left is c5's Windows half alone.

Getting c4 cost more than a CI push, and the finding outlives it. `8d6add71`,
a throwaway red-arm instrument whose own commit body says *"Delete this branch
after reading the run; it is an instrument, not a fix"*, had been MERGED into
`integ/f13` as `d03a6e14`. It cuts `OwnedTree::snapshot` short behind
`black_box(true)`, so the guard owned the LEAF only — on every Unix, across all
forty-plus swept sites, in the integration tree. c1 and c2 stayed green the
whole time, because they certify that every site HANDS its child to the guard,
not what the guard then does with it. `bf0b41f7` reverts the ten lines.

`#358` still holds the Windows half. It landed a kill-on-close Job Object
rather than a descendant walk, so `child_pids` there returns an empty vector by
design and c5's stated settlement condition needs restating in those terms.
