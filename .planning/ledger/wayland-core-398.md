---
issue: 398
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate makes a guard on any path named objects/modules/store cost one syscall per workspace directory (split from #390 c3)"
status: open
last_verified_commit: 972d1c17c
criteria:
  - id: c1
    text: "A warm guard on a gate-admitted path costs a number of filesystem probes INDEPENDENT of the workspace's directory count, stated as a number -- vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory INVERTED (slope 0) rather than deleted"
    state: not-met
    owner: core
    note: "STAYS not-met, re-measured on this tree at 972d1c17c after core#390 c2 closed and UNCHANGED: GATE COST: dirs=8 admitted=19 ordinary=3 | dirs=48 admitted=59 ordinary=3, slope exactly 1.000, gate-refused control flat at 3 at both sizes. The ATTRIBUTION changed and it is what makes this closeable on its own: the slope is store_shaped`s alone, and store_shaped no longer decides what arm 3 can refuse - it survives only as the net for a store created or renamed since the last walk. See c5."
  - id: c2
    text: "The gate-refused ordinary path is unchanged at one resolution / zero warm scans / three warm probes: an_ordinary_path_never_pays_for_the_nested_store_walk stays green, stated as a number"
    state: not-met
    owner: core
    note: "NOT met and it MOVED, in the COLD direction only. The warm figure this criterion says to HOLD is held exactly - one resolution, zero warm scans, three warm probes, at both workspace sizes. The cold one-off changed: with the arm-3 gate decided by the scan`s own output (core#390 c2), the walk runs once per policy on the FIRST guard instead of on the first store-shaped path, so the first guard scans twice and costs 43 probes where it cost 12 on the vfs_guard_cost fixture (12 root scan + 31 walk, named FIRST_GUARD_PROBES). The test is renamed an_ordinary_path_pays_for_the_nested_store_walk_at_most_once and grades the steady state as a difference over N guards, which a one-off cannot inflate; its known-positive control is now that the ONE walk already paid for is the walk that refuses the vendored store, with no further scan. Related, same direction: nested_stores_memoized no longer drops the memo above DENY_CACHE_MAX_DIRS - with the gate reading that memo, refusing to keep one would re-walk the tree on EVERY guard instead of on the store-shaped ones alone."
  - id: c3
    text: "The per-traversed-directory figure grep_policy::scope_for pays does not exceed the 5.000 total syscalls/directory measured at #390's merge, re-measured by differential strace at three operation counts with the probe's known-positive control asserted green in every run"
    state: not-met
    owner: core
    note: "Nothing done to the code; the BASELINE this must hold is re-derived and stated. Three DETACHED worktrees with private target/ dirs, differential strace -f -c over probe_vcs_content_stores_per_traversed_directory at WL_PROBE_DIRS 100/1100/2100, `1 passed` asserted in all nine runs, binary discrimination asserted by sha256 (all three share one cargo metadata hash wcore_tools-78a8b0c9297025a9, so the filename proves nothing) -- 9d036b03fffeaed830331962 / 766e6fbe3943a0fa663532a4 / f8af101f7ff07112c098cfbb -- and by symbol presence (discover_nested_content_stores 0/0/5, StoreScan 0/24/30, store_shaped 0/0/8). TOTAL syscalls per traversed directory, both intervals exact in every arm: 8.000 at 4c55f5ac6 (7 statx + 1 openat), 17.000 at 1b9cb34d5 (16 statx + 1 openat), 5.000 at this tree (5 statx, no openat)."
  - id: c4
    text: "The memo still fails CLOSED: a store created after the scan is denied on the next guard, and a symlinked control directory's late-created store is denied -- both tests stay green"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_store_created_after_the_scan_is_denied_on_the_next_guard
    owner: core
    note: "MET at 972d1c17c and asserted, not assumed: a_store_created_after_the_scan_is_denied_on_the_next_guard, an_alternates_borrow_written_after_the_scan_is_denied, a_store_under_a_symlinked_control_dir_created_after_the_scan_is_denied and a_store_leaf_symlinked_to_a_later_created_directory_is_denied are all green in a 1854-test wcore-tools run, and all four went RED under the c2 red arm (nested_walk_admits reverted to store_shaped alone), so they are load-bearing rather than incidentally green. The invariant was NOT traded away to flatten c1`s slope: the witness set is unchanged, and the change that could have weakened it - keeping the memo above DENY_CACHE_MAX_DIRS - keeps every witness rather than dropping any."
  - id: c5
    text: "Whether this is closed jointly with FerroxLabs/wayland-core#394 is decided explicitly and recorded"
    state: met
    evidence: symbol:crates/wcore-tools/src/workspace_policy.rs::nested_walk_admits
    owner: core
    note: "DECIDED 2026-08-31 and recorded: they do NOT close jointly, and core#394 is now closed with this ticket`s slope untouched. The coupling this criterion recorded was real but was about the fix that was ASSUMED - mutating store_shaped to true - not the one that landed. core#390 c2 closed by making the gate a function of the scan`s OWN OUTPUT (nested_walk_admits), which costs zero filesystem probes, so the admitted-path slope is unchanged at 1.000 and #394`s correctness gap is gone. Consequence for this ticket: c1 is now a question about store_shaped`s REMAINING role alone - the post-scan-mutation net - and that role, plus the residual it leaves, is tracked as FerroxLabs/wayland-core#406. c1 cannot be closed by deleting store_shaped without answering #406 c1 first, because deleting it removes the only thing that catches a store renamed since the walk."
---

Split out of core#390 c3 while re-grading it. #390 closed the correctness half
(a vendored gitfile's object store is now refused through the VFS); this is the
COST half of the gate that fix had to introduce.

Arm 3 walks the workspace to discover nested control directories. Discovery is
expensive, so arm 3 is reached only for a path `store_shaped` admits — one
carrying a `VCS_CONTENT_STORES` leaf name among its components (`objects`,
`modules`, `lfs`, `store`, `pristine`, `repository`). Those are ordinary project
directory names: a Terraform `modules/`, a Redux `store/`, an asset `objects/`.
A workspace containing no nested checkout anywhere still has paths that open the
gate, and every guard on one revalidates the nested walk's witness set — the walk
stamps one witness per directory it descended, and `cache_hit` stats every
witness on every call.

`SecretDenyFs` is installed unconditionally for every sub-agent and every
channel/remote session, and sub-agents are read-heavy. On a 10,000-directory
checkout one `Read` of `modules/vpc/main.tf` costs ~10,000 `symlink_metadata`
calls where it cost 3 before #390, and it is paid again on every subsequent
operation on any such path, because revalidation is the steady state rather than
the cold scan. Not a correctness hole: the answers are right and the failure
direction is slow rather than stale.

Criteria are taken verbatim from the issue's Acceptance section.
