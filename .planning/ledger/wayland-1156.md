---
issue: 1156
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: acp serve survives its parent and reparents to PPID 1 — 9 orphans found, oldest 24h, pinning 160GB"
status: open
last_verified_commit: e7144c30a
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
    note: "All five sites the ledger named now wrap the spawn in OwnedTree: profile_router_live.rs:118, headless_acp_boot.rs:165 and :351, f21_02_01_child_tool_authority.rs:416, f21_02_child_budget_live.rs:361. No remaining unbound acp-serve spawn under crates/wcore-cli/tests. The ~40 OTHER json-stream spawn sites are wayland-core#352 and the Windows leaf-only degradation is wayland-core#358; neither is in scope here. RE-GRADED 2026-08-29 at e7144c30a on hetzner-dsm (Linux). The previous NOT-CLOSEABLE grade was correct at the time and its cause is gone: 8d6add71 (`RED ARM (throwaway, never merge): leaf-only OwnedTree on macOS`) reached integ/f13 through the merge d03a6e14 and put `if std::hint::black_box(true) { return; }` at the top of `#[cfg(unix)] OwnedTree::snapshot()`, so `known` was always empty and the guard owned only the leaf; 8df19170 removed it and `grep -c black_box crates/wcore-cli/tests/support/owned_tree.rs` is now 0. `cargo nextest run -p wcore-cli --test harness_owns_spawned_trees` -> `Summary [0.224s] 24 tests run: 24 passed, 0 skipped`, with the keyed test dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child PASS in 0.223s. NON-VACUITY, my own red arm at this commit rather than the inherited one: re-inserted the same `if std::hint::black_box(true) { return; }` as the first statement of the `#[cfg(unix)] snapshot()` BODY (the edited region was printed back afterwards, so the mutation landed on CODE inside the fn, not on the doc comment above it), touched the file, and the keyed test went RED on both attempts -- verbatim `thread 'dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child' (2388661) panicked at crates/wcore-cli/tests/harness_owns_spawned_trees.rs:121:5: the grandchild 2388663 outlived the guard -- killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)`, `Summary [20.225s] 24 tests run: 23 passed, 1 failed`. Restored with `git checkout --` + `touch`; baseline back to 24/24 in 0.317s and `git status --porcelain` empty. PLATFORM SCOPE -- this Unix green must NOT be read as more than it is. harness_owns_spawned_trees.rs is `#![cfg(unix)]` (line 30), so it grades ZERO Windows behaviour: the Windows arm owns the tree through a Job Object and `OwnedTree::snapshot()` is an empty no-op there (owned_tree.rs:361), which this file cannot even compile. Within Unix the run above is LINUX ONLY -- `descendants()` is cfg-gated to target_os linux over /proc (owned_tree.rs:75) and refuses to fall back to pgrep; macOS takes a separate pgrep walk (owned_tree.rs:101-106) that no merged run has graded green (measuring it is what 8d6add71 was built for, and that result was never landed). c2 is MET ON LINUX; macOS and Windows are ungraded by this evidence and stay that way until a macOS CI arm runs this file."
---

The product half is fixed in v0.13.10; the half this ticket asked for was
not, and the distinction matters.

The evidence in the report was a surviving SERVER with a surviving child. The
fix kills the child. Nothing kills the server. Five test sites still spawn
`acp serve` unbound, so the same nine-orphan pile-up is still reachable from
the test suite on the build host.

Grading c1 as "fixed" and closing would be exactly the substitution — a real
fix, to an adjacent problem, presented as the reported one.
