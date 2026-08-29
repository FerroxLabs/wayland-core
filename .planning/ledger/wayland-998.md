---
issue: 998
repo: FerroxLabs/wayland
kind: defect
title: "Per-tool switches in the MCP Library are inert on Wayland Core and ACP backends"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A tool the operator switched off is not registered at boot"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_per_tool_allowlist.rs::a_denied_tool_is_not_registered"
    owner: core
  - id: c2
    text: "The denial survives a mid-session tools/list_changed refresh"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_per_tool_allowlist.rs::the_denial_survives_a_list_changed_refresh"
    owner: core
  - id: c3
    text: "An ABSENT allowlist still registers every tool, and Some([]) means none rather than being folded into absent"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_per_tool_allowlist.rs::an_absent_allowlist_registers_every_tool"
    owner: core
  - id: c4
    text: "The enforcement holds under defer_config_mcp — the mode Desktop actually runs"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_deferred_servers_empty_allowlist_survives_the_refresh"
    owner: core
    note: "The old gate is gone: bootstrap.rs:3528 installs the refresh unconditionally and engine.rs:5189 no longer returns early on an empty one. integrate_deferred_mcp (main.rs:3887) admits the deferred manager WITH its server_configs through McpCatalogRefresh::register_runtime_server, which REFUSES an empty config map (tool_proxy.rs:429-441) precisely so a refresh cannot hit the config==None allow-all read and restore the full tool set. The cited test drives the real integrate_deferred_mcp with allowed_tools = Some([]) and asserts the locked tool is still absent after a live list_changed. Landed with wayland#1174."
  - id: c5
    text: "Desktop sends the per-tool field on the ACP path"
    state: blocked
    owner: desktop
    note: "Desktop's wire types drop the field before it reaches core, so core cannot honour a selection it never receives"
  - id: c6
    text: "The ACP backend has an MCP surface for the switches to act on"
    state: met
    evidence: "test:crates/wcore-cli/tests/acp_mcp_tool_selection.rs::a_switched_off_mcp_tool_is_not_offered_over_acp"
    owner: core
    note: "Built. `McpToolSelection { server, allowed_tools }` (symbol:crates/wcore-acp/src/protocol.rs::McpToolSelection, Desktop's `allowedTools` accepted as an alias) rides `SessionCreateRequest::mcp_servers`, is stored on the session record, and is read from THERE on every turn onto `TurnRequest::mcp_servers` (test:crates/wcore-acp/src/server.rs::session_create_mcp_switches_reach_the_turn_engine). `message/send` carries no MCP field at all, so a later message can neither introduce a selection nor widen one. symbol:crates/wcore-cli/src/acp_engine.rs::narrow_mcp_tool_selection applies it to the Config the engine is built from, BEFORE bootstrap dials, so a denied tool is never registered rather than merely hidden. Version skew is negotiated via symbol:crates/wcore-acp/src/protocol.rs::ServerCapabilities.mcp_tool_selection because the request types are deny_unknown_fields. Production caller: `wayland-core acp request create-session --mcp-tools <server>=<tool,tool>`. SECURITY: strictly authority-REDUCING - it names a server the operator already configured and intersects with what that config allowed, and wcore-acp carries no field for a command, URL, header or credential, so a client can never declare a server (test:crates/wcore-cli/tests/acp_mcp_tool_selection.rs::a_selection_can_never_widen_what_the_config_allowed). RED ARM, run: replacing the `narrow_mcp_tool_selection` call with `let _ = mcp_selection;` reddened 4 of the 6 cases, e.g. `the tool the operator switched OFF must not be offered; offered: [... \"safe_read\", \"danger_delete\"]`. Restored + touched, 29/29 green."
---

Core-side enforcement is complete. The TICKET is not, and the safety framing
is still live: someone who switches off a destructive tool in the MCP Library
still believes they disabled it.

Three independent reasons it stays open are above. c4 is the sharpest — the
enforcement is real, and it is bypassed by the exact mode Desktop runs in.

Note for whoever takes #1174/#1175: they are load-bearing on each other.
Fixing #1175 without threading `McpServerConfig` into `server_configs` would
make the allow-all read at `tool_proxy.rs:447` go live and restore a server's
full tool set on `list_changed`. That must be one change.
