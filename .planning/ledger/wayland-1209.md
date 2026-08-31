---
issue: 1209
repo: FerroxLabs/wayland
kind: defect
title: "With builtin_tools.defer_cold.catalog = false, ToolSearch hydration still re-sorts tools[] mid-array: #1171 is fully live on that path"
status: closed
last_verified_commit: 4e4f9d53f
criteria:
  - id: c1
    text: "In stub mode a hydration leaves the wire prefix stable: the measured turn1/turn2 arrays no longer differ at index 1"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::stub_mode_hydration_leaves_the_engine_tools_prefix_byte_identical"
    owner: core
    note: "MET. The fix is on lane/f13-w3-host-protocol and reached this tree by merge, not by re-implementation: `sink_deferred_to_tail` now runs UNCONDITIONALLY in `apply_tool_deferral`, ahead of `admit_hydrated_tools`, so both catalog modes share one ordering discipline and a hydration can only mutate the tail region. Verified in this tree by `cargo nextest run -p wcore-agent --test tools_prefix_stable_on_hydration` (green) after the merge. GRADED, NOT INHERITED: the merge took the peer lane's engine_bridge.rs and main.rs sides over integ/f13's, so the claim had to be re-checked against the merged tree rather than carried across."
  - id: c2
    text: "A test asserts prefix stability under hydration in BOTH catalog modes, with the catalog=true path as the positive control; shown RED against today's stub mode"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::stub_mode_hydration_leaves_the_engine_tools_prefix_byte_identical"
    owner: core
    note: "MET. The guard is engine-driven, which is the half the previous evidence could not do: the test calls `AgentEngine::apply_tool_deferral` and hydrates through the engine's own `record_called_deferred_tool` recorder, so a revert at the single production site reddens it. Both modes in one test with catalog=true as the positive control. Green in this tree post-merge."
  - id: c3
    text: "If the shift is deliberate in stub mode, the config key's documentation says that turning the fold off also gives up cache stability, and the user is told at load rather than billed silently"
    state: met
    evidence: "symbol:crates/wcore-config/src/tools.rs::DeferColdConfig"
    owner: core
    note: "MET by removing the antecedent rather than satisfying it: the shift is no longer deliberate in stub mode, so there is nothing to warn about at load. The config key's doc comment states the shared ordering discipline and names both engine-driven guard tests."
---

With the documented config knob `builtin_tools.defer_cold.catalog = false` (DeferColdConfig, crates/wcore-config/src/tools.rs:45-49, `false` restores per-tool stub entries), a ToolSearch hydration still rewrites the tools[] wire prefix mid-array — i.e. defect #1171 is fully live on that path. In stub mode `fold_deferred_into_catalog` is skipped (engine.rs:20104, guarded by `defer_cfg.enabled && defer_cfg.catalog`), so deferred tools remain in the array at their registry slots; `admit_hydrated_tools` then REMOVES each hydrated stub from mid-array and appends it at the tail, shifting everything after it. MEASURED on hetzner with a scratch probe crate (repo untouched): turn1 [Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch, Workflow, Write] -> turn2 [Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write, Delegate, Spawn, Workflow]; `prefix stable: false`, `first differing wire index: Some(1)` — literally the index-1 shift the ticket cites. Positive control in the same run: the default catalog=true path IS prefix-stable, so this is not a harness artefact.

**Where.** crates/wcore-agent/src/engine.rs:20093-20109 (apply_tool_deferral) + crates/wcore-tools/src/registry.rs:572 (admit_hydrated_tools) + crates/wcore-config/src/tools.rs:45-49 (DeferColdConfig::catalog). Reachable via user config `[builtin_tools.defer_cold] catalog = false`.

**Why it matters.** A user who turns off the catalog fold is opting out of a token optimisation, not out of cache stability — but they silently get the full #1171 re-bill (~6,000 uncached tokens on the measured leader) on every session that touches a deferred tool, including every Spawn. Nothing warns them and no test covers it: the only catalog=false test, engine.rs::catalog_mode_emits_no_stub_entries_and_config_off_restores_stubs, checks stub presence, never prefix stability under hydration. Fix shape: in stub mode, either leave the hydrated stub at its slot and only flip `deferred=false` in place (the stub and the full schema differ in bytes anyway, but the shift is avoided for everything after it), or run the tail-move unconditionally so the two modes share one ordering discipline.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
