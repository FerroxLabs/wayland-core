---
issue: 1175
repo: FerroxLabs/wayland
title: "A runtime-added MCP server's tools/list_changed is ignored for the life of the session"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "The outcome chosen is 'honoured', not 'say plainly it will not be'. A source lint over main.rs and tui/engine_bridge.rs asserting the register_runtime_server call count and the forget_runtime_server rollback. All three paths are wired: main.rs:3887 (deferred config), main.rs:5702 (AddMcpServer), tui/engine_bridge.rs:3043 (/mcp add), with :3117 forgetting on rollback."
  - id: c2
    text: "A test adds a server at runtime, has it announce a new tool, and asserts that tool becomes callable"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_dynamic_tools.rs::a_runtime_added_server_is_refreshed_alongside_the_boot_servers"
    owner: core
    note: "Rollback direction covered by a_withdrawn_runtime_server_stops_being_refreshed."
  - id: c3
    text: "A manager created after boot can be registered with McpCatalogRefresh"
    state: met
    evidence: "symbol:crates/wcore-mcp/src/tool_proxy.rs::register_runtime_server"
    owner: core
    note: "McpCatalogRefresh's fields are now Mutex-wrapped (tool_proxy.rs:375-378), so the '&self methods only, no post-construction registration' obstacle the ticket named is gone. Returns bool and refuses on empty configs."
---

Every runtime-add path builds a brand-new `McpManager` that never enters the
boot-captured `McpCatalogRefresh.managers`. Only boot managers are polled, so a
server added after startup can never have a `tools/list_changed` honoured, and a
tool it late-registers stays uncallable for the rest of the session. Three paths
are confirmed: the `AddMcpServer` command, the deferred config MCP integration
from #551, and the TUI `/mcp add`. All three park their manager in
`dynamic_managers`, which reaches nothing but read-only diagnostics.

`/mcp add` is the documented way to attach a server mid-session, so the
documented path is the broken one.

c3 is listed separately because it is the structural obstacle rather than the
symptom — there is currently no API by which any fix could register a late
manager, so this cannot be closed by wiring alone. Not a security issue: the
per-tool allowlist is never dropped, and the `config == None` allow-all read in
`tool_proxy.rs` was investigated and found unreachable for any server carrying
an operator allowlist. #1174 is the broader sibling case.
