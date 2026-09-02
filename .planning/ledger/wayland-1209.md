---
issue: 1209
repo: FerroxLabs/wayland
kind: defect
title: "With builtin_tools.defer_cold.catalog = false, ToolSearch hydration still re-sorts tools[] mid-array: #1171 is fully live on that path"
status: closed
last_verified_commit: 56b54a06e
criteria:
  - id: c1
    text: "In stub mode a hydration leaves the wire prefix stable: the measured turn1/turn2 arrays no longer differ at index 1"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::stub_mode_hydration_leaves_the_engine_tools_prefix_byte_identical"
    owner: core
    note: "MET, and RE-VERIFIED IN THIS TREE by its own red arm rather than carried across a merge. `sink_deferred_to_tail` runs UNCONDITIONALLY in `AgentEngine::apply_tool_deferral` (engine.rs:20909), ahead of `admit_hydrated_tools`, so both catalog modes share one ordering discipline and a hydration can only mutate the tail region. GREEN: `cargo nextest run -p wcore-agent -E 'test(stub_mode_hydration_leaves_the_engine_tools_prefix_byte_identical) + test(both_catalog_modes_agree_on_the_engine_hot_prefix)'` -> `2 tests run: 2 passed`."
  - id: c2
    text: "A test asserts prefix stability under hydration in BOTH catalog modes, with the catalog=true path as the positive control; shown RED against today's stub mode"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::stub_mode_hydration_leaves_the_engine_tools_prefix_byte_identical"
    owner: core
    note: "MET, and the vacuity the sweep reported is CLOSED AND MEASURED. The old guard (tests/tools_prefix_stable_on_hydration.rs) re-composed the helper sequence BY HAND and stayed green with the production step deleted; the live guard calls `apply_tool_deferral` itself and hydrates through `record_called_deferred_tool`. RED ARM RUN HERE, not quoted from an earlier lane: deleted the `wcore_tools::registry::sink_deferred_to_tail(&mut tools);` line at engine.rs:20909, `touch`ed engine.rs, `cargo check -p wcore-agent --tests` RC=0 (so the mutation COMPILED), then the two guards -> `2 tests run: 0 passed, 2 failed`, the first panicking with `wayland#1209: the hydration turn rewrote wire index 1 / turn 1: [Bash, Delegate, Edit, ...] / turn 2: [Bash, Edit, Forge, ...]` -- the ticket's measured shape. CONTROL ON THE OLD GUARD, in the same red arm: tools_prefix_stable_on_hydration stayed `2 tests run: 2 passed`, which is the vacuity reproduced rather than asserted. Restored, `touch`ed, both guards green again. Both catalog modes are in the one test with catalog=true as the positive control."
  - id: c3
    text: "If the shift is deliberate in stub mode, the config key's documentation says that turning the fold off also gives up cache stability, and the user is told at load rather than billed silently"
    state: met
    evidence: "symbol:crates/wcore-config/src/tools.rs::DeferColdConfig"
    owner: core
    note: "MET by removing the antecedent rather than satisfying it: the shift is no longer deliberate in stub mode, so there is nothing to warn about at load. `DeferColdConfig::catalog`'s doc comment (crates/wcore-config/src/tools.rs:46-63) states the shared ordering discipline, says in terms that turning the fold off costs TOKENS and not cache stability, and names both engine-driven guards -- read and confirmed in this tree."
---

With the documented config knob `builtin_tools.defer_cold.catalog = false` (DeferColdConfig, crates/wcore-config/src/tools.rs:45-49, `false` restores per-tool stub entries), a ToolSearch hydration still rewrites the tools[] wire prefix mid-array — i.e. defect #1171 is fully live on that path. In stub mode `fold_deferred_into_catalog` is skipped (engine.rs:20104, guarded by `defer_cfg.enabled && defer_cfg.catalog`), so deferred tools remain in the array at their registry slots; `admit_hydrated_tools` then REMOVES each hydrated stub from mid-array and appends it at the tail, shifting everything after it. MEASURED on hetzner with a scratch probe crate (repo untouched): turn1 [Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch, Workflow, Write] -> turn2 [Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write, Delegate, Spawn, Workflow]; `prefix stable: false`, `first differing wire index: Some(1)` — literally the index-1 shift the ticket cites. Positive control in the same run: the default catalog=true path IS prefix-stable, so this is not a harness artefact.

**Where.** crates/wcore-agent/src/engine.rs:20093-20109 (apply_tool_deferral) + crates/wcore-tools/src/registry.rs:572 (admit_hydrated_tools) + crates/wcore-config/src/tools.rs:45-49 (DeferColdConfig::catalog). Reachable via user config `[builtin_tools.defer_cold] catalog = false`.

**Why it matters.** A user who turns off the catalog fold is opting out of a token optimisation, not out of cache stability — but they silently get the full #1171 re-bill (~6,000 uncached tokens on the measured leader) on every session that touches a deferred tool, including every Spawn. Nothing warns them and no test covers it: the only catalog=false test, engine.rs::catalog_mode_emits_no_stub_entries_and_config_off_restores_stubs, checks stub presence, never prefix stability under hydration. Fix shape: in stub mode, either leave the hydrated stub at its slot and only flip `deferred=false` in place (the stub and the full schema differ in bytes anyway, but the shift is avoided for everything after it), or run the tail-move unconditionally so the two modes share one ordering discipline.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
