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
    state: not-met
    owner: core
    note: "Re-verified against 43848f75: wcore-acp has NO MCP surface at all. Grepping the crate for mcp returns three incidental comments (client.rs:59, client.rs:368 buffer-size analogies; idempotency.rs:17 naming the JSON-stream command), and crates/wcore-cli/src/acp.rs contains the string zero times. There is no route, type or field for the switches to act on."
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
