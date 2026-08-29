---
issue: 1209
repo: FerroxLabs/wayland
kind: defect
title: "With builtin_tools.defer_cold.catalog = false, ToolSearch hydration still re-sorts tools[] mid-array: #1171 is fully live on that path"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "In stub mode a hydration leaves the wire prefix stable: the measured turn1/turn2 arrays no longer differ at index 1"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D27, found while verifying FerroxLabs/wayland#1171 — ToolSearch hydration re-sorts tools[] mid-array, invalidating the prompt cache). Nothing has been done. The measured finding, verbatim: With the documented config knob `builtin_tools.defer_cold.catalog = false` (DeferColdConfig, crates/wcore-config/src/tools.rs:45-49, `false` restores per-tool stub entries), a ToolSearch hydration still rewrites the tools[] wire prefix mid-array — i.e. defect #1171 is fully live on that path. In stub mode `fold_deferred_into_catalog` is skipped (engine.rs:20104, guarded by `defer_cfg.enabled && defer_cfg.catalog`), so deferred tools remain in the array at their registry slots; `admit_hydrated_tools` then REMOVES each hydrated stub from mid-array and appends it at the tail, shifting everything after it. MEASURED on hetzner with a scratch probe crate (repo untouched): turn1 [Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch, Workflow, Write] -> turn2 [Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write, Delegate, Spawn, Workflow]; `prefix stable: false`, `first differing wire index: Some(1)` — literally the index-1 shift the ticket cites. Positive control in the same run: the default catalog=true path IS prefix-stable, so this is not a harness artefact."
  - id: c2
    text: "A test asserts prefix stability under hydration in BOTH catalog modes, with the catalog=true path as the positive control; shown RED against today's stub mode"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D27). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "If the shift is deliberate in stub mode, the config key's documentation says that turning the fold off also gives up cache stability, and the user is told at load rather than billed silently"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D27). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

With the documented config knob `builtin_tools.defer_cold.catalog = false` (DeferColdConfig, crates/wcore-config/src/tools.rs:45-49, `false` restores per-tool stub entries), a ToolSearch hydration still rewrites the tools[] wire prefix mid-array — i.e. defect #1171 is fully live on that path. In stub mode `fold_deferred_into_catalog` is skipped (engine.rs:20104, guarded by `defer_cfg.enabled && defer_cfg.catalog`), so deferred tools remain in the array at their registry slots; `admit_hydrated_tools` then REMOVES each hydrated stub from mid-array and appends it at the tail, shifting everything after it. MEASURED on hetzner with a scratch probe crate (repo untouched): turn1 [Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch, Workflow, Write] -> turn2 [Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write, Delegate, Spawn, Workflow]; `prefix stable: false`, `first differing wire index: Some(1)` — literally the index-1 shift the ticket cites. Positive control in the same run: the default catalog=true path IS prefix-stable, so this is not a harness artefact.

**Where.** crates/wcore-agent/src/engine.rs:20093-20109 (apply_tool_deferral) + crates/wcore-tools/src/registry.rs:572 (admit_hydrated_tools) + crates/wcore-config/src/tools.rs:45-49 (DeferColdConfig::catalog). Reachable via user config `[builtin_tools.defer_cold] catalog = false`.

**Why it matters.** A user who turns off the catalog fold is opting out of a token optimisation, not out of cache stability — but they silently get the full #1171 re-bill (~6,000 uncached tokens on the measured leader) on every session that touches a deferred tool, including every Spawn. Nothing warns them and no test covers it: the only catalog=false test, engine.rs::catalog_mode_emits_no_stub_entries_and_config_off_restores_stubs, checks stub presence, never prefix stability under hydration. Fix shape: in stub mode, either leave the hydrated stub at its slot and only flip `deferred=false` in place (the stub and the full schema differ in bytes anyway, but the shift is avoided for everything after it), or run the tail-move unconditionally so the two modes share one ordering discipline.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
