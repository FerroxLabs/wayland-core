---
issue: 398
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate makes a guard on any path named objects/modules/store cost one syscall per workspace directory (split from #390 c3)"
status: open
last_verified_commit: c680860b3
criteria:
  - id: c1
    text: "A warm guard on a gate-admitted path costs a number of filesystem probes INDEPENDENT of the workspace's directory count, stated as a number -- vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory INVERTED (slope 0) rather than deleted"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by the w3-vcs-residuals lane while RE-GRADING core#390 c3 from met to not-met -- the cost half of the gate the #390 fix had to introduce, and a regression against the tree #390 landed on. MEASURED through GuardCounters, not timed, so the figures are identical on every platform: a warm guard on <root>/modules/vpc/main.tf (no control directory, gitfile or store anywhere under it) costs 19 probes in an 8-directory workspace and 59 in a 48-directory one -- a slope of exactly 1.000 probe per workspace directory -- while the gate-REFUSED path stays at 3 in both as the control. A/B re-derived independently 2026-08-30 by copying vfs_guard_cost.rs verbatim into a DETACHED worktree at 1b9cb34d5 (the tree #390 landed on) with its own target/: `left: 0, right: 40`, printing `GATE COST: dirs=8 admitted=3 ordinary=3 | dirs=48 admitted=3 ordinary=3`. So the slope is arm 3's and not the fixture's."
  - id: c2
    text: "The gate-refused ordinary path is unchanged at one resolution / zero warm scans / three warm probes: an_ordinary_path_never_pays_for_the_nested_store_walk stays green, stated as a number"
    state: not-met
    owner: core
    note: "Nothing done; this is the number to HOLD, not to move. Green today at cddc3d9df. It is also the reason c1 cannot be closed by widening store_shaped: mutating store_shaped to `true` reddens this test in the same run as it reddens c1's."
  - id: c3
    text: "The per-traversed-directory figure grep_policy::scope_for pays does not exceed the 5.000 total syscalls/directory measured at #390's merge, re-measured by differential strace at three operation counts with the probe's known-positive control asserted green in every run"
    state: not-met
    owner: core
    note: "Nothing done to the code; the BASELINE this must hold is re-derived and stated. Three DETACHED worktrees with private target/ dirs, differential strace -f -c over probe_vcs_content_stores_per_traversed_directory at WL_PROBE_DIRS 100/1100/2100, `1 passed` asserted in all nine runs, binary discrimination asserted by sha256 (all three share one cargo metadata hash wcore_tools-78a8b0c9297025a9, so the filename proves nothing) -- 9d036b03fffeaed830331962 / 766e6fbe3943a0fa663532a4 / f8af101f7ff07112c098cfbb -- and by symbol presence (discover_nested_content_stores 0/0/5, StoreScan 0/24/30, store_shaped 0/0/8). TOTAL syscalls per traversed directory, both intervals exact in every arm: 8.000 at 4c55f5ac6 (7 statx + 1 openat), 17.000 at 1b9cb34d5 (16 statx + 1 openat), 5.000 at this tree (5 statx, no openat)."
  - id: c4
    text: "The memo still fails CLOSED: a store created after the scan is denied on the next guard, and a symlinked control directory's late-created store is denied -- both tests stay green"
    state: not-met
    owner: core
    note: "Nothing done; this is the invariant a cheaper revalidation must not trade away. Green today at cddc3d9df. Recorded as a criterion because the obvious way to flatten c1's slope is to stamp fewer witnesses, and a memo whose invalidation cannot observe the mutation is not a cache -- workspace_policy.rs says so in its own words."
  - id: c5
    text: "Whether this is closed jointly with FerroxLabs/wayland-core#394 is decided explicitly and recorded"
    state: not-met
    owner: core
    note: "Nothing decided. MEASURED coupling, so the decision is not a judgement call about tidiness: mutating store_shaped to `true` -- the widening that would close #394 -- reddens a_gate_admitted_path_costs_one_probe_per_workspace_directory, an_ordinary_path_never_pays_for_the_nested_store_walk and one_ordinary_path_guard_resolves_once_and_does_not_rescan in the same run. #394's own body already names the shape that closes both: resolve borrow targets eagerly at scan time into a set the point-predicate tests by prefix with no lexical pre-gate."
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
