---
issue: 1156
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: acp serve survives its parent and reparents to PPID 1 — 9 orphans found, oldest 24h, pinning 160GB"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "acp serve profile children are bound to a parent-death channel, so the product does not leave the reported orphans"
    state: met
    evidence: "symbol:crates/wcore-cli/src/parent_channel.rs::watch_for_orphaning"
    owner: core
    note: "shipped and live-proven"
  - id: c2
    text: "The TEST SUPERVISOR owns the tree — no test site spawns acp serve unbound"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees.rs::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child"
    owner: core
    note: "All five sites the ledger named now wrap the spawn in OwnedTree: profile_router_live.rs:118, headless_acp_boot.rs:165 and :351, f21_02_01_child_tool_authority.rs:416, f21_02_child_budget_live.rs:361. No remaining unbound acp-serve spawn under crates/wcore-cli/tests. The ~40 OTHER json-stream spawn sites are wayland-core#352 and the Windows leaf-only degradation is wayland-core#358; neither is in scope here."
---

The product half is fixed in v0.13.10; the half this ticket asked for was
not, and the distinction matters.

The evidence in the report was a surviving SERVER with a surviving child. The
fix kills the child. Nothing kills the server. Five test sites still spawn
`acp serve` unbound, so the same nine-orphan pile-up is still reachable from
the test suite on the build host.

Grading c1 as "fixed" and closing would be exactly the substitution — a real
fix, to an adjacent problem, presented as the reported one.
