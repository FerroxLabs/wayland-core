---
issue: 1156
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: acp serve survives its parent and reparents to PPID 1 — 9 orphans found, oldest 24h, pinning 160GB"
status: open
last_verified_commit: 9de21aa1
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
    note: "All five sites the ledger named now wrap the spawn in OwnedTree: profile_router_live.rs:118, headless_acp_boot.rs:165 and :351, f21_02_01_child_tool_authority.rs:416, f21_02_child_budget_live.rs:361. No remaining unbound acp-serve spawn under crates/wcore-cli/tests. The ~40 OTHER json-stream spawn sites are wayland-core#352 and the Windows leaf-only degradation is wayland-core#358; neither is in scope here. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: The evidence test resolves AND IS RED at origin/integ/f13. Ran `cargo nextest run -p wcore-cli --test harness_owns_spawned_trees` on hetzner: `Summary [20.156s] 24 tests run: 23 passed, 1 failed`, failing twice (TRY 1 + TRY 2) with the ticket's own words: 'the grandchild 4130539 outlived the guard — killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)'. Cause: `crates/wcore-cli/tests/support/owned_tree.rs:346-355` contains a committed RED ARM — `if std::hint::black_box(true) { return; }` as the first statement of `#[cfg(unix)] OwnedTree::snapshot()`. `known` is therefore always empty on Unix, `kill_all(&known)` kills nothing, and the guard owns only the LEAF — precisely the ownership the ticket measured as insufficient ('a whole process tree survived, not just a leaf'). Its own commit says so: 8d6add71 'RED ARM (throwaway, never merge): leaf-only OwnedTree on macOS [ci-darwin]' ... 'Delete this branch after reading the run; it is an instrument, not a fix.' It was merged into integ/f13 anyway at d03a6e14 (2026-08-29 13:25 UTC) and is still on the remote tip origin/integ/f13 @ e151392e. CONTROL (A/B, mutation landed on CODE not a comment — I removed only the three-line `if` block, left the comment intact, `touch`ed the file, built into a separate target dir from a `git archive HEAD` copy, modified nothing in sweep-base): 24/24 PASS, key test green in 0.243s. So the red arm is the sole cause. The ledger's own note is otherwise accurate — all five named sites ARE wrapped (`profile_router_live.rs:118`, `headless_acp_boot.rs:165` and `:351`, `f21_02_01_child_tool_authority.rs:416`, `f21_02_child_budget_live.rs:361`) and the #352 ratchet `every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard` PASSES — but wrapping a neutered guard is not ownership. Additionally: the ledger verified at `last_verified_commit: 43848f75` (2026-08-29 06:43), which PREDATES the red-arm merge at 13:25; nobody re-graded after it. Platform: the evidence test is `#![cfg(unix)]`; the Windows Job Object arm and the macOS `pgrep` descendant walk are ungraded by it (the macOS arm is what the red arm was built to grade, and that run never produced a merged green). RE-GRADED AT HEAD 9de21aa1: the refutation above no longer holds. Its sole named cause -- the committed red arm at crates/wcore-cli/tests/support/owned_tree.rs -- was removed by 8df191706 'Remove a red arm that shipped, and arm a gate that could never pass'. Re-run on hetzner at this tree: 'cargo nextest run -p wcore-cli --test harness_owns_spawned_trees' -> 'Summary [0.227s] 24 tests run: 24 passed, 0 skipped', with dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child PASS in 0.226s. c2 therefore stays met. The platform half of the sweep's note stands and is NOT covered here: the evidence test is #![cfg(unix)], and the macOS pgrep arm and the Windows Job Object arm are graded by core#352 c4/c5 and core#358, all of which remain not-met."
---

The product half is fixed in v0.13.10; the half this ticket asked for was
not, and the distinction matters.

The evidence in the report was a surviving SERVER with a surviving child. The
fix kills the child. Nothing kills the server. Five test sites still spawn
`acp serve` unbound, so the same nine-orphan pile-up is still reachable from
the test suite on the build host.

Grading c1 as "fixed" and closing would be exactly the substitution — a real
fix, to an adjacent problem, presented as the reported one.
