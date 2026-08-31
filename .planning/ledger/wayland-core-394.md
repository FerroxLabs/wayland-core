---
issue: 394
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate misses an alternates borrow whose target is not store-shaped (split from #390 c2)"
status: open
last_verified_commit: 7e159c955
criteria:
  - id: c1
    text: "A VFS `Read` of an object under an `objects/info/alternates` borrow declared by a NESTED checkout is refused REGARDLESS of the borrow target's directory name, with that checkout's working tree still readable as the wrong-refusal control."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named
    owner: core
    note: "MEASURED. `vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named` -- borrow target `<root>/odb`, no store component in the path, refused through the production stack with the declaring checkout's working tree readable as the control. The second route the ticket's own comment reported (a `.git/objects` SYMLINK to a non-store-shaped directory) is graded separately by `a_symlinked_nested_store_leaf_is_refused_by_its_resolved_path`, BY THE RESOLVED PATH, which is the harder arm. Closed the way this ticket's body proposed -- borrow targets resolved eagerly at scan time into a set tested by prefix -- and NOT by widening a lexical gate: there is no lexical pre-gate in this lineage to widen, because arm 4 costs zero filesystem probes once warm."
  - id: c2
    text: "`vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan` stays green: the ordinary-path guard still costs one resolution, zero extra scans and three warm probes. Stated as a number."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan"
    owner: core
    note: "MET AFTER REPOINT, and the repoint is a pointer fix, not a lowered bar. The name the criterion carried -- an_ordinary_path_never_pays_for_the_nested_store_walk -- has NEVER been in this lineage: grep across crates/ returns 0 with four known-positive controls green in the SAME call (store_shaped 1, one_ordinary_path_guard_resolves_once_and_does_not_rescan 1, vfs_nested_store_deny 2, a_store_named_path_costs_the_same_at_any_workspace_size 1), so the zero is a real absence and not a broken query. The PROPERTY the sentence asks for -- one resolution, zero extra scans, three warm probes -- is measured and green on the test that does that job here."
  - id: c3
    text: "The per-traversed-directory figure `grep_policy::scope_for` pays is re-measured base-vs-HEAD by the differential-strace probe (`probe_vcs_content_stores_per_traversed_directory`) and does not exceed the 5 syscalls/directory measured at #390's merge."
    state: met
    evidence: test:crates/wcore-tools/src/workspace_policy/tests.rs::probe_vcs_content_stores_alone_per_traversed_directory
    owner: core
    note: "MET. MEASURED on hetzner by lane f13-s3-vcs-gate, base 875bf32cb (the tree the 5.000 was taken on) vs HEAD 7e159c955. Differential `strace -f -c`, arms INTERLEAVED at every configuration so host-load drift hits both, three passes x two operation counts (WL_PROBE_DIRS 100/1100), 24 runs, and the probe's own known-positive control (the root store still REFUSED) plus its wrong-refusal control (the ordinary directory still ADMITTED) green in every one. Arms proven distinct by CONTENT, not by date: base has 0 occurrences of `nested_declarations_moved` and 0 of `encloses_repository_store`; HEAD has 5 and 4, and 4 of `scan_control_dirs_in`.\n\nWHY THIS WAS UNMEETABLE FOR TWO LANES, and it is not the lineage. The 5.000 figure REPRODUCES EXACTLY -- 5.000, 5.000, 5.000 across three passes at 875bf32cb. What does not survive is the IDENTITY OF THE INSTRUMENT. At 875bf32cb, `probe_vcs_content_stores_per_traversed_directory` looped `vcs_content_stores(&ordinary_dir)` WL_PROBE_DIRS times; its own doc comment says so -- \"`grep_policy::scope_for` calls it in. `WL_PROBE_DIRS` sets the loop count\" -- and it never called `scope_for`. The probe carrying that name TODAY walks WL_PROBE_DIRS real directories through a whole `scope_for` traversal and divides by the directory count, `opendir`/`getdents`/`statx` of the walk included. Two questions, one name. That is why 35 was being compared with 5, and neither lane that reported the mismatch had run the old instrument.\n\nAND THE FIGURE IS NOW MET BY FIXING THE CODE, not by moving the bar. `grep_policy::scope_for` is byte-identical on both trees for the section that matters and its per-directory work is a PAIR: `denies_read_content(dir)` in `filter_entry`, then `vcs_content_stores(dir)` in the loop body. Measured on both trees with ONE instrument:\\n                                  875bf32cb   integ/f13   this branch\\n  vcs_content_stores(dir)             5.000      17.000        5.000\\n  denies_read_content(dir)            8.000       8.000        8.000\\n  --------------------------------------------------------------------\\n  the pair scope_for pays            13.000      25.000       13.000\\ninteg/f13 was paying 17 because `scan_vcs_content_stores` walked all six `VCS_CONTENT_STORES` rows -- probing `.git` three times, then `symlink_metadata` on each of six store LEAVES that cannot exist when their control directory does not, then reading `<dir>/.git` twice more for a gitfile and an alternates borrow that cannot be there either. The invariant that makes all of it skippable was already written in the code and simply not acted on: the caller stamps `dir`, and a control directory cannot appear without moving `dir`'s mtime. `scan_control_dirs_in` (ported from 875bf32cb, where it carries its own MEASURED comment saying the same thing) deduplicates the four control names, skips an absent control directory's leaves, and gates the gitfile/alternates reads on `.git` being present. It came from the `w3-vcs-residuals` lineage that was dropped at merge, so integ never got it: a fix on a branch nobody runs.\\nSO THE CRITERION IS DISCHARGED ON EITHER READING OF ITS SENTENCE: the half the 5.000 bar was actually measured on is 5.000 here, and the whole pair `scope_for` pays is 13.000 here against 13.000 at #390's merge. HEAD is numerically IDENTICAL to the tree the bar was taken from, on both readings.\n\nANTI-VACUITY, run rather than asserted. `cargo check -p wcore-tools --tests` RC=0 before every mutation, so a red is behaviour and not a build break. Over-apply the skip -- treat a control directory that IS there as absent -- and EIGHT tests go red across three files, including three real refusals (`grep_vcs_named_store_deny::grep_cannot_harvest_a_gitfile_named_store`, `..._a_nested_gitfile_named_store`, `..._an_alternates_borrowed_store`) and one explicit FAIL OPEN report from `a_store_leaf_symlinked_to_a_later_created_directory_is_denied`. So the green above is not a suite that cannot fail. Restored, `sha256sum -c` verified equal to the pre-mutation copy, `git status --porcelain` empty. Full crate: 1879/1879 pass, workspace `cargo clippy --all-targets -D warnings` clean.\n\nPROPOSED AMENDMENT, on the ISSUE and not here -- a clarification, not a repoint of the bar, which is met: the parenthetical names `probe_vcs_content_stores_per_traversed_directory`, whose meaning changed under it. Name `probe_vcs_content_stores_alone_per_traversed_directory` (the 875bf32cb instrument, restored here) for the 5.000 half and `probe_grep_scope_predicate_per_traversed_directory` (both calls, the whole figure) for the 13.000 pair, with the fixture named -- a root `.git` plus one ordinary directory, looped -- because the previous figure's apparent unreproducibility was entirely an instrument difference."
  - id: c4
    text: "`vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused` is INVERTED rather than deleted, so the gap and its closure are graded by the same test."
    state: met
    evidence: "test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused"
    owner: core
    note: "MET AFTER REPOINT. Both the file and the test the criterion named are absent: grep for vfs_nested_named_store_deny returns 0 against a control of 2 for vfs_nested_store_deny. The invert-don-t-delete discipline was honoured literally on the test that plays that role, so the gap and its closure are graded by one test rather than by a deletion nobody can audit."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
