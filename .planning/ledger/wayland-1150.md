---
issue: 1150
repo: FerroxLabs/wayland
title: "[Bug]: Absurd Input Token Size"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "An unlisted model no longer gets a fabricated 200,000-token window; it sizes from the bottom of the range"
    state: met
    evidence: "symbol:crates/wcore-config/src/limits.rs::model_output_ceiling"
    owner: core
  - id: c2
    text: "Not every tool and MCP server is sent on every prompt"
    state: met
    evidence: "symbol:crates/wcore-tools/src/registry.rs::admit_hydrated_tools"
    owner: core
    note: "satisfied by MCP curation and tool deferral that shipped BEFORE this release, not by anything in it. Credit where due, but do not credit 0.13.10 for it"
  - id: c3
    text: "Large fetched content is truncated or summarised before it enters the context"
    state: not-met
    owner: core
    note: "WEB_FETCH_MAX_RESPONSE_BYTES = 256 * 1024 (crates/wcore-tools/src/web_fetch.rs:78) passes a 50,000-character result through untouched"
---

Partially fixed in v0.13.10.

The compaction half landed: a model that is not in the limits table no longer
gets handed a fabricated 200,000-token window, which was silently
under-compacting and producing the input sizes this reporter saw.

c2 is recorded as met-but-not-by-this-release deliberately. Grading it as an
outstanding ask would be wrong; grading it as something 0.13.10 delivered
would be a false claim. Both are avoidable by saying which release did it.

c3 is the live remainder and is concrete: a quarter-megabyte web fetch enters
the context untouched.
