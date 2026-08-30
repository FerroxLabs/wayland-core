---
issue: 1209
repo: FerroxLabs/wayland
kind: defect
title: "With builtin_tools.defer_cold.catalog = false, ToolSearch hydration still re-sorts tools[] mid-array: #1171 is fully live on that path"
status: closed
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "In stub mode a hydration leaves the wire prefix stable: the measured turn1/turn2 arrays no longer differ at index 1"
    state: met
    evidence: "test:crates/wcore-agent/tests/tools_prefix_stable_on_hydration.rs::hydration_leaves_the_tools_prefix_byte_identical_in_both_catalog_modes"
    owner: core
    note: "sink_deferred_to_tail runs unconditionally in apply_tool_deferral, ahead of admit_hydrated_tools, so a hydration can only mutate the tail region. The test asserts names[1] is equal across turn1/turn2 -- the exact index the ticket measured -- AND that the serialized bytes of the whole hot prefix are identical, so a Vec-order-only fix would not pass it."
  - id: c2
    text: "A test asserts prefix stability under hydration in BOTH catalog modes, with the catalog=true path as the positive control; shown RED against today's stub mode"
    state: met
    evidence: "test:crates/wcore-agent/tests/tools_prefix_stable_on_hydration.rs::hydration_leaves_the_tools_prefix_byte_identical_in_both_catalog_modes"
    owner: core
    note: "One test drives both modes: catalog=false is the arm under test (asserted to be genuinely stub mode -- 11 wire entries, at least one (Deferred) description) and catalog=true is the positive control, which holds the property before and after this change. RED, RUN not asserted: `sink_deferred_to_tail` (crates/wcore-tools/src/registry.rs) reduced to a compiling no-op -- `cargo check -p wcore-tools --tests` exit 0, so the red is a behaviour change and not a build failure -- reddens this test at tools_prefix_stable_on_hydration.rs:219 with `wayland#1209: the hydration turn rewrote wire index 1`, turn 1 [Bash, Delegate, Edit, ...] vs turn 2 [Bash, Edit, Forge, ...], the exact index-1 shift the ticket measured, and reddens both_modes_agree_on_the_hot_prefix as well. The other two tests in the file (hydration_appends_and_leaves_the_tools_prefix_byte_identical, a_second_hydration_appends_after_the_first) stay GREEN under the same mutation, so the arm discriminates rather than breaking the file. Restored; git hash-object == git rev-parse HEAD:<path> = d4eef5640."
  - id: c3
    text: "If the shift is deliberate in stub mode, the config key's documentation says that turning the fold off also gives up cache stability, and the user is told at load rather than billed silently"
    state: met
    evidence: "symbol:crates/wcore-config/src/tools.rs::DeferColdConfig"
    owner: core
    note: "The antecedent is removed rather than satisfied: the shift is NOT deliberate in stub mode any more, so there is nothing to warn about at load. The config key's doc comment states the shared ordering discipline and names the guard test, so a future change cannot reintroduce the interleave believing stub mode never had cache stability to give up."
---

With the documented config knob `builtin_tools.defer_cold.catalog = false` (DeferColdConfig, crates/wcore-config/src/tools.rs:45-49, `false` restores per-tool stub entries), a ToolSearch hydration still rewrites the tools[] wire prefix mid-array — i.e. defect #1171 is fully live on that path. In stub mode `fold_deferred_into_catalog` is skipped (engine.rs:20104, guarded by `defer_cfg.enabled && defer_cfg.catalog`), so deferred tools remain in the array at their registry slots; `admit_hydrated_tools` then REMOVES each hydrated stub from mid-array and appends it at the tail, shifting everything after it. MEASURED on hetzner with a scratch probe crate (repo untouched): turn1 [Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch, Workflow, Write] -> turn2 [Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write, Delegate, Spawn, Workflow]; `prefix stable: false`, `first differing wire index: Some(1)` — literally the index-1 shift the ticket cites. Positive control in the same run: the default catalog=true path IS prefix-stable, so this is not a harness artefact.

**Where.** crates/wcore-agent/src/engine.rs:20093-20109 (apply_tool_deferral) + crates/wcore-tools/src/registry.rs:572 (admit_hydrated_tools) + crates/wcore-config/src/tools.rs:45-49 (DeferColdConfig::catalog). Reachable via user config `[builtin_tools.defer_cold] catalog = false`.

**Why it matters.** A user who turns off the catalog fold is opting out of a token optimisation, not out of cache stability — but they silently get the full #1171 re-bill (~6,000 uncached tokens on the measured leader) on every session that touches a deferred tool, including every Spawn. Nothing warns them and no test covers it: the only catalog=false test, engine.rs::catalog_mode_emits_no_stub_entries_and_config_off_restores_stubs, checks stub presence, never prefix stability under hydration. Fix shape: in stub mode, either leave the hydrated stub at its slot and only flip `deferred=false` in place (the stub and the full schema differ in bytes anyway, but the shift is avoided for everything after it), or run the tail-move unconditionally so the two modes share one ordering discipline.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
