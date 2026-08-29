---
issue: 1171
repo: FerroxLabs/wayland
kind: defect
title: "ToolSearch hydration re-sorts tools[] mid-array, invalidating the prompt cache on the turn after any deferred-tool load"
status: closed
last_verified_commit: 3262536a
criteria:
  - id: c1
    text: "Hydration APPENDS to tools[] instead of re-sorting it, so the serialized prefix is byte-identical across a hydration"
    state: met
    evidence: "test:crates/wcore-agent/tests/tools_prefix_stable_on_hydration.rs::hydration_appends_and_leaves_the_tools_prefix_byte_identical"
    owner: core
    note: "asserts the serialized PREFIX BYTES, not the Vec order — a Vec-order assertion would pass on an encoder that re-sorts"
  - id: c2
    text: "Wire order is [stable base] ++ [hydrated, first-hydration order] ++ [ToolSearch], in one place"
    state: met
    evidence: "symbol:crates/wcore-tools/src/registry.rs::admit_hydrated_tools"
    owner: core
  - id: c3
    text: "A hydrated tool is not un-deferred in place, and the deferred-tool catalogue does not ride on ToolSearch's description mid-array"
    state: met
    evidence: "test:crates/wcore-tools/src/registry.rs::admit_hydrated_tools_leaves_an_already_hot_tool_in_place"
    owner: core
  - id: c4
    text: "With the documented knob `builtin_tools.defer_cold.catalog = false` a hydration still leaves the full-schema wire prefix byte-identical"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::catalog_off_keeps_the_hot_wire_prefix_stable_across_a_hydration"
    owner: core
    note: "c1/c2 hold on the catalog path only. In stub mode fold_deferred_into_catalog is skipped, so the stubs stay at their REGISTRY slots interleaved with the hot tools and admit_hydrated_tools lifts one out of mid-array -- MEASURED first differing wire index 1, i.e. the whole cached prefix re-billed on any session that touches a deferred tool, Spawn included. partition_deferred_to_tail hoists the full-schema defs ahead of the stubs before the admission, so both modes now share one ordering rule. Bounded claim: a stub becoming a full schema must change bytes somewhere; the fix makes that somewhere the deferred tail rather than index 1"
  - id: c5
    text: "The same discipline still holds in catalog mode, so neither mode can be fixed by breaking the other"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::catalog_on_keeps_the_hot_wire_prefix_stable_across_a_hydration"
    owner: core
    note: "asserts every wire entry ahead of the ToolSearch catalog carrier is byte-identical across a hydration"
---

Closed in v0.13.10. It was three faults, not one: all four wire encoders
sorted `tools[]` by name (turning an append into a mid-array insert),
`apply_tool_deferral` un-deferred a hydrated tool in place, and the
deferred-tool catalogue rode on `ToolSearch`'s description in the middle of
the array.

One deliberate trade, recorded because it is a real loss: the encoders no
longer guarantee invariance to input reordering. That guarantee and
append-only prefix stability are mutually exclusive — the sort bought the
former by making every append an insert.

Honest limit: this pays on implicit-prefix wires (OpenAI chat/Responses,
Gemini). On Anthropic a single `cache_control` breakpoint sits on the LAST
tool, so any change to the array rewrites that zone wherever it lands. No
token-delta claim is made, because none was measured.
