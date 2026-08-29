---
issue: 1150
repo: FerroxLabs/wayland
title: "[Bug]: Absurd Input Token Size"
status: open
last_verified_commit: 43848f75
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
    state: met
    evidence: "test:crates/wcore-tools/src/web_fetch.rs::a_large_fetched_page_does_not_enter_the_context_whole"
    owner: core
    note: "WEB_FETCH_MAX_TEXT_CHARS = 20,000 caps text on a char boundary, flips truncated and adds truncation_notice; max_result_size was raised above the tool's own worst case so orchestration::truncate_result cannot mangle the JSON envelope. Negative control a_page_under_the_cap_is_untouched_and_not_marked_truncated is present."
  - id: c4
    text: "Accumulated prior tool RESULTS are not re-sent whole on every turn, and prompt/KV cache is reused where possible"
    state: not-met
    owner: core
    note: "Both are named in the reporter's own Expected Behavior and neither had a criterion. c2 covers tool and MCP DEFINITIONS only; c3 bounds ONE WebFetch result and does nothing about N of them accumulating across a session. Prompt-cache work is tracked separately under wayland#559 and #1168 but nothing pins it for this ticket."
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
