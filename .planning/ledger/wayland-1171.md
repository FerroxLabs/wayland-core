---
issue: 1171
repo: FerroxLabs/wayland
kind: defect
title: "ToolSearch hydration re-sorts tools[] mid-array, invalidating the prompt cache on the turn after any deferred-tool load"
status: closed
last_verified_commit: 43848f75
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
