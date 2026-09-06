---
issue: 1328
repo: FerroxLabs/wayland
kind: defect
title: "Core contract: expose effective tool approvals and preserve per-tool default inheritance (#1188)"
status: open
last_verified_commit: 3d2089b45
criteria:
  - id: c1
    text: "Publish a typed, session-scoped effective tool-policy/inventory snapshot with canonical tool IDs, registration state, effective approval disposition, and source/provenance. Re-emit or revise it after mode/config/registry changes."
    state: not-met
    owner: core
    note: "Requested producer contract not implemented or verified by this metadata task."
  - id: c2
    text: "Include engine-derived mode effects so Desktop can remove its Write/Edit AutoEdit mirror and stop treating a curated static catalog as runtime availability."
    state: not-met
    owner: core
    note: "Core implementation remains pending; existing workspace filesystem policy does not establish tool approval or registration truth."
  - id: c3
    text: "Changing one tool while the base allow-list is absent preserves effective decisions for untouched tools and future inherited defaults through an inheritance-preserving override or equivalent bounded mutation contract."
    state: not-met
    owner: core
    note: "Requested bounded mutation contract pending; a seeded whole-list replacement is not evidence."
  - id: c4
    text: "Pin producer fixtures and negative tests for absent versus explicit-empty lists, default/AutoEdit/Force modes, unregistered tools, and stale session revisions; replaying the snapshot lets Desktop render effective decisions without a default or mode-specific tool-name literal."
    state: not-met
    owner: core
    note: "Producer fixtures and paired consumer proof remain pending. Older producers must stay explicitly unknown; keep filesystem access separate from approval and registration."
---

Recorded from live #1328 on 2026-09-06. Tracker is OPEN, area:core, needs:core, with no milestone. Dependency for Desktop #1188; no implementation or milestone change is authorized by recording this issue. Classified defect conservatively because the body names duplicated authority and accidental materialization of inherited configuration.
