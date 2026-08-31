---
issue: 398
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate makes a guard on any path named objects/modules/store cost one syscall per workspace directory (split from #390 c3)"
status: open
last_verified_commit: 30fd6cfde
criteria:
  - id: c1
    text: "— The warm per-guard cost of a gate-admitted path is INDEPENDENT of the workspace's directory count. `vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory` grades this as a slope and currently pins the slope at 1.0; when it is fixed that assertion is INVERTED rather than deleted, so the regression and its closure are graded by the same test."
    state: not-met
    owner: core
    note: "The named test (`vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory`) is not in this lineage and cannot be inverted here. The PROPERTY is met and measured: `vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size` (added here, in the inverted form the criterion asks for) measures the warm per-guard probe count for `<root>/modules/vpc/main.tf` -- no control dir, gitfile or store beneath it -- in two workspaces differing only by ordinary directories: 4 probes at 8 directories and 4 at 48. SLOPE 0, against the 1.000 this ticket reports. Zero by construction, not by tuning: this lineage never introduces a lexical pre-gate on a whole-tree walk, because arm 4 answers from a set built once and never revalidated (`nested_walk_count()` asserted == 1 after 21 guards). RED ARM named in the test's own doc: make `nested_content_stores` rebuild per call and the slope returns."
  - id: c2
    text: "— The refusals #390 bought stay bought: `vfs_nested_named_store_deny.rs` stays green in full, including the vendored-gitfile arm and the nested-alternates arm, so the cost fix is not a quiet revert."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::one_ordinary_path_guard_resolves_once_and_does_not_rescan
    owner: core
    note: "`one_ordinary_path_guard_resolves_once_and_does_not_rescan` green: 1 resolution per guard, `scans` stays at 1 across 51 guards, warm probes exactly N*3 = 3 per guard. Unchanged from base. The COLD one-off moved and is stated rather than waved past: the first guard now costs 41 probes where it cost 17 on this fixture, because arm 4's walk runs once on the first guard. `nested_walks` is counted APART from `scans` so the two cannot be conflated, and the test asserts it stays at 1."
  - id: c3
    text: "— The ordinary (gate-refused) path is unchanged at one resolution / zero warm scans / three warm probes, still pinned by `one_ordinary_path_guard_resolves_once_and_does_not_rescan`."
    state: not-met
    owner: core
    note: "See #394 c3 / #396 c3: the 5.000 baseline is from a tree that is not an ancestor of `integ/f13`. Re-measured base-vs-HEAD here by differential `strace -f -c` at WL_PROBE_DIRS 100/1100 x WL_PROBE_REPS 1/6, `1 passed` and the known-positive control green in every one of the eight runs: steady-state 35.009 (base) -> 34.997 (HEAD) syscalls per traversed directory. The +6.0/directory seen at REPS=1 is the arm-4 one-off and is proven one-off by the fact that it does not repeat across REPS."
  - id: c4
    text: "— The figure is re-stated as a number for BOTH call shapes on the tree the fix lands on, base-vs-HEAD, by the two instruments named above (the counted slope and the differential `strace` per-traversed-directory figure), each with its known-positive control green."
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_guard_cost.rs::a_store_named_path_costs_the_same_at_any_workspace_size
    owner: core
    note: "Both call shapes, base-vs-HEAD, each with its control:\n* counted slope (`GuardCounters`, platform-independent): gate-admitted path 4 probes at 8 dirs and 4 at 48 dirs, slope 0; gate-refused ordinary path 3 warm probes at both -- the control that shows the fixture is not the reason.\n* differential `strace` per traversed directory: base 35.009 -> HEAD 34.997, probe's known-positive control asserted green in every run.\nCold one-off stated separately: first guard 17 -> 41 probes on the `vfs_guard_cost` fixture, +6.0 syscalls/workspace directory, once per policy."
  - id: c5
    text: "— The `DENY_CACHE_MAX_DIRS` branch of `nested_stores_memoized` is graded by a test or the branch is removed; today no test reaches it and its cost behaviour is asserted only in a comment."
    state: not-met
    owner: core
    note: "The branch the criterion names does not exist in this lineage: `nested_stores_memoized` is on `lane/f13-w3-vcs-residuals`, not an ancestor of `integ/f13`. Arm 4 here is a `std::sync::OnceLock` with NO size-conditional caching branch, so there is no such branch to grade or remove -- the failure mode the criterion guards against (a workspace above `DENY_CACHE_MAX_DIRS` re-walking on every guard) is unreachable by construction. RECORDED AND STILL OPEN: the unrelated `DENY_CACHE_MAX_DIRS` branch in #1111's `deny_cache` (`workspace_policy.rs`, `(!dirs.is_empty() && dirs.len() <= DENY_CACHE_MAX_DIRS).then(...)`) is still reached by no test in this tree. That is a different memo and this lane did not touch it.\nThe ticket's other c5 (whether this closes jointly with #394) is DECIDED and the answer is YES, by one mechanism: arm 4 costs zero warm probes, so it needs no lexical gate, so #394's gate-blindness and #398's gate cost are the same absence. The measured coupling this ticket recorded -- widening `store_shaped` reddens the cost tests -- does not arise, because `store_shaped` is never introduced."
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
