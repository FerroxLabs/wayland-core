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
    note: "MERGED into integ/next as 2165c30a on 2026-08-29. The sweep covers every .spawn() and .spawn_command() site under crates/wcore-cli/tests; ALLOWED_UNOWNED is EMPTY, which is the ticket's 'no unstated remainder', and MINIMUM_KNOWN_SITES = 40 floors the scan so a reader that has stopped seeing the tree cannot pass by seeing nothing. 04681f33 added the last one (the PTY probe in quarantine_terminal_authority). CORRECTION 2026-08-29: this criterion was MET and simultaneously NOT TRUE of the integration tree, and the sweep is why nobody noticed. 8d6add71, a throwaway red-arm instrument whose own commit body says 'Delete this branch after reading the run; it is an instrument, not a fix', was merged into integ/f13 as d03a6e14. It returns from OwnedTree::snapshot behind std::hint::black_box(true), so the descendant list is never populated and the guard owns the LEAF only -- the exact pre-#1156 behaviour, applied to all forty-plus swept sites at once, on every Unix rather than only macOS. The cited sweep still passed throughout, because it reads the crate's test SOURCES for spawn expressions handed to OwnedTree and every site still hands its child over; what changed was what the guard then does with it. That is the shape of hole this criterion should be read as leaving: it certifies the WIRING, not the mechanism the wiring reaches. Reverted on this branch by bf0b41f7, which removes those ten lines and nothing else; harness_owns_spawned_trees goes 24/24 green on hetzner-dsm."
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
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees.rs::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child"
    owner: core
    note: "MET 2026-08-29. The macOS pgrep descendant walk is EXECUTED and GREEN in CI, cited: https://github.com/FerroxLabs/wayland-core/actions/runs/33257637102 (job 99114021304), branch lane/f13-352-macos-green2 at the head of this lane, KEPT and not deleted so the run stays chaseable. Verbatim: >>> PASS [   0.175s] ( 8121/17178) wcore-cli::harness_owns_spawned_trees dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child / Summary [1145.585s] 17178 tests run: 17177 passed (1 leaky), 1 failed, 120 skipped <<< That is the arm running against the unmutated guard on macos-latest -- pgrep -P, because macOS has no /proc -- and it is the citation the previous passes could not produce. GETTING IT REQUIRED FINDING A DEFECT FIRST, and that is the part worth keeping. A green run was impossible on integ/f13: the throwaway instrument 8d6add71 had been MERGED as d03a6e14, so OwnedTree::snapshot returned early behind black_box(true) and the guard owned the leaf only. Measured on the unmodified integration tree rather than inferred -- hetzner-dsm, 3 of 3 tries in the full nextest sweep and 3 of 3 again with the test run ALONE, so not scheduling and not load: >>> thread 'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child' panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: the grandchild 1658408 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156) <<< and on macOS in https://github.com/FerroxLabs/wayland-core/actions/runs/33255873933 (job 99109355513, branch lane/f13-352-macos-green, also kept), same test, same line, grandchild 83274. Bisected: green at bb850cc5^ (0df4c47d), red at ab6b602f. bf0b41f7 reverts the ten lines. NOTE WHERE THE RED FELL, because it settles this criterion twice over: line 121, the ownership assertion, NOT the line 89 precondition child_pids(direct).contains(&grandchild). The pgrep walk ran on macOS and FOUND the grandchild even while the snapshot was cut, so the arm is executed on the red run as well as on the green one. ONE PRE-EXISTING RED SHARES THE CITED LEG and is not this criterion's: wcore-protocol::quiescence_contract::the_published_corpus_is_current is the only other failure in the whole macOS leg, and it is Desktop-contract-corpus drift over seven files (events/ready.json, manifest.json, three adversarial/events/*.jsonl, two compat/events/*). Its Linux sibling is wcore-protocol::desktop_contract_corpus::checked_corpus_matches_real_serializers_byte_for_byte. Both are red at ab6b602f BEFORE any commit on this lane and green at 0df4c47d, verified by running them alone in clean worktrees at each commit, so they are an integration defect this lane inherited and did not cause."
  - id: c5
    text: "A red arm is quoted verbatim for each platform arm"
    state: not-met
    owner: core
    note: "NOT MET, and precisely one arm short. The Windows arm has no red arm and cannot have one until #358 lands, which is the whole of the remaining gap. The other two arms now have a verbatim red arm and the macOS one is cited by run URL, which is what the previous pass could not find. macOS -- CI (macos-latest) in https://github.com/FerroxLabs/wayland-core/actions/runs/33244213309 (job 99078722412), branch lane/f13-352-macos-redarm at 8d6add71, a THROWAWAY instrument branch that is deliberately NOT deleted so the evidence stays chaseable. That branch reduces OwnedTree::snapshot to leaf-only ownership behind std::hint::black_box(true), which keeps the rest of the body reachable so clippy -D warnings still passes -- the previous macOS attempt (run 33235055214) died on an unrelated lint before it reached a test, and the fmt and clippy steps of this one are green. Verbatim: `thread 'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child' (408307) panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: / the grandchild 2235 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)` / `TRY 1 FAIL [  10.204s]`, `TRY 2 FAIL [  10.168s]`, `TRY 3 FAIL [  10.085s]` / `Summary [1079.158s] 16950 tests run: 16949 passed (3 leaky), 1 failed, 118 skipped` -- EXACTLY ONE test failed in the whole macOS leg, so the red is attributable to the mutation and to nothing else. NOTE WHAT ELSE THAT RUN PROVES: it reddens at line 121, not at the line 89 precondition `child_pids(direct).contains(&grandchild)`, so the pgrep -P descendant walk RAN on macOS and FOUND the grandchild. The macOS arm is executed, not merely compiled. Linux -- two arms run on hetzner-dsm 2026-08-29 for comparison. Withdrawing the /proc walk (child_pids -> Vec::new()) reddens at the PRECONDITION instead: `panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:89:5: / the grandchild 1076605 must be visible as a descendant of 1076604; saw []`. That is the shape Windows is in TODAY, and it is the reason c5's Windows half cannot be closed here: with child_pids returning an empty vector this test refuses to grade tree ownership at all rather than failing the ownership assertion, so there is no red arm to quote and no green one either. Withdrawing only the snapshot (the same mutation the macOS branch carries) reddens at the ownership assertion on Linux too: `panicked at .../harness_owns_spawned_trees.rs:121:5: / the grandchild 1132275 outlived the guard -- ...`. WHAT WOULD SETTLE THE WINDOWS ARM: #358 landing a real descendant walk in owned_tree.rs::child_pids under cfg(windows), then one `CI (Array)` / self-hosted Windows leg on a lane branch carrying the same black_box leaf-only mutation. Until then the Windows arm is unreddenable by construction, not unattempted. CORRECTION 2026-08-29: 'a THROWAWAY instrument branch that is deliberately NOT deleted so the evidence stays chaseable' was true of the branch and false of what happened to it -- lane/f13-352-macos-redarm was MERGED into integ/f13 as d03a6e14, so the leaf-only mutation shipped in the integration tree and disabled the guard on every Unix until bf0b41f7 reverted it (see c1 and c4). The macOS red arm quoted above still stands. It now has a corroborating second one that needed NO mutation at all, which is strictly stronger: https://github.com/FerroxLabs/wayland-core/actions/runs/33255873933 job 99109355513, macos-latest on the unmodified integration tree, same test, same line 121, grandchild 83274. Also: the Windows settlement condition stated above is now the wrong shape. #358 landed a kill-on-close Job Object rather than a descendant walk, so `known` stays empty there BY DESIGN and the reddening mutation is withdrawing the job assignment, not the snapshot."
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
