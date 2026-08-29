---
issue: 998
repo: FerroxLabs/wayland
title: "Per-tool switches in the MCP Library are inert on Wayland Core and ACP backends"
status: open
last_verified_commit: cfa89a9c
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
    state: not-met
    owner: core
    note: "bootstrap.rs:1810 gates the config-MCP connect on !defer_config_mcp, so under it no catalog refresher is installed at all and mid-session list_changed is invisible. Filed as #1174"
  - id: c5
    text: "Desktop sends the per-tool field on the ACP path"
    state: blocked
    owner: desktop
    note: "Desktop's wire types drop the field before it reaches core, so core cannot honour a selection it never receives"
  - id: c6
    text: "The ACP backend has an MCP surface for the switches to act on"
    state: not-met
    owner: core
    note: "the 'ACP half' is a flat per-turn tool list; wcore-acp has no MCP surface at all"
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
