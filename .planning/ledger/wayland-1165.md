---
issue: 1165
repo: FerroxLabs/wayland
title: "Opt-in: /mcp add --replace to deliberately reconfigure a connected server"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "An explicit opt-in exists that tears down and re-establishes a connected MCP server's connection deliberately"
    state: not-met
    owner: core
    note: "spelled /mcp add --replace or equivalent in the issue; nothing of the kind exists in the tree"
  - id: c2
    text: "Without the opt-in, re-adding a ready server still leaves its configuration and generation untouched"
    state: met
    evidence: "test:crates/wcore-agent/src/mcp_lifecycle.rs::ready_readd_keeps_existing_generation_even_when_config_changes"
    owner: core
    note: "this is the #605 Gap 3 guard the new flag must not weaken — a duplicate add must never silently mutate a live server"
---

Split out of #605 at close. #605 deliberately left reconfigure-on-re-add
unimplemented and guarded, because a re-add must never silently mutate a
connected server's configuration. This issue tracks the explicit opt-in for the
user who genuinely wants that: `/mcp add --replace`, which tears down and
re-establishes the connection on purpose rather than as a side effect of a
duplicate add.

It is a feature, not a defect, and it was filed so the intent is tracked rather
than left implied inside a closed bug. Nothing has been built.

c2 is the existing behaviour the feature has to preserve, and it is met today:
re-adding a ready server whose config identity has changed returns the existing
snapshot with its generation unchanged. It is recorded here so that a future
`--replace` implementation is graded against keeping the default safe, not only
against making the new flag work.
