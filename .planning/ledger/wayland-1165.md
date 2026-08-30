---
issue: 1165
repo: FerroxLabs/wayland
kind: feature
title: "Opt-in: /mcp add --replace to deliberately reconfigure a connected server"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "An explicit opt-in exists that tears down and re-establishes a connected MCP server's connection deliberately"
    state: met
    evidence: "test:crates/wcore-cli/tests/mcp_replace_e2e.rs::replace_tears_the_connection_down_and_re_establishes_it"
    owner: core
    note: "Built in both spellings the issue names. json-stream: `AddMcpServer { replace: bool }` (default false, skip-serialized so the pre-#1165 wire is byte-identical; test:crates/wcore-protocol/src/commands.rs::add_mcp_server_replace_is_opt_in_and_absent_by_default), handled by symbol:crates/wcore-cli/src/main.rs::teardown_runtime_mcp_for_replace, which RELEASES the name (registry unregister, close_server, lifecycle stopping/complete) so the ordinary reserve then mints a fresh generation - the c2 catalog invariant is routed around by an explicit remove-then-add, never weakened. TUI: `/mcp add --replace <name> <target>` via symbol:crates/wcore-cli/src/tui/engine_bridge.rs::parse_mcp_add + symbol:crates/wcore-cli/src/tui/engine_bridge.rs::replace_mcp_server, built from the same two halves /mcp restart already is. Refuses rather than interrupts a connecting/stopping/cleanup-unverified server, and refuses if cleanup cannot be verified rather than connecting a second child beside a process nobody proved dead. OBSERVED: the cited e2e drives the packaged binary over json-stream against a stdio fixture that logs its pid and names its one tool after its argv - it asserts the first child EXITS, a second child is serving, and the tool set changed alpha -> beta. RED ARM, run: disabling the teardown block turned the replace back into the pre-#1165 refusal - `the replace must connect, not refuse: {\"reason\":\"same-name MCP server is already owned by a different configuration; remove it before re-adding\"}` - while the default-path guard below stayed green. Restored + touched."
  - id: c2
    text: "Without the opt-in, re-adding a ready server still leaves its configuration and generation untouched"
    state: met
    evidence: "test:crates/wcore-agent/src/mcp_lifecycle.rs::ready_readd_keeps_existing_generation_even_when_config_changes"
    owner: core
    note: "this is the #605 Gap 3 guard the new flag must not weaken — a duplicate add must never silently mutate a live server. STILL MET after c1: `replace` is default-false and acts only when set, and the catalog's reserve() is untouched. Re-graded end to end at the process level by test:crates/wcore-cli/tests/mcp_replace_e2e.rs::a_plain_re_add_still_changes_nothing, which re-adds a DIFFERENT configuration under a live name without the opt-in and asserts the refusal, no second child, and the original child still alive."
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
