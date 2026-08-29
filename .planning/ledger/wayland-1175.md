---
issue: 1175
repo: FerroxLabs/wayland
kind: defect
title: "A runtime-added MCP server's tools/list_changed is ignored for the life of the session"
status: open
last_verified_commit: ca211126
criteria:
  - id: c1
    text: "An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be"
    state: met
    evidence: "test:crates/wcore-mcp/tests/list_changed_non_stdio_transports.rs::the_manager_picks_up_a_tool_an_sse_server_registers_mid_session"
    owner: core
    note: "The outcome chosen is 'honoured', not 'say plainly it will not be'. The runtime-ADD wiring was already met (a source lint over main.rs and tui/engine_bridge.rs asserting the register_runtime_server call count and the forget_runtime_server rollback; all three paths wired at main.rs:3887, main.rs:5702, tui/engine_bridge.rs:3043 with :3117 forgetting on rollback), but the close-sweep found the criterion FALSE for two of three transports: take_tools_changed() defaulted to false and was overridden only by StdioTransport, so refresh_signalled_tools could never fire for an SSE or Streamable-HTTP server. Closed as a class: the predicate now lives once in transport/mod.rs, SseTransport raises it from its listener (which also now drains what the handshake already read -- an announcement in the same TCP segment as the endpoint event was parked unparsed forever), and StreamableHttpTransport raises it from BOTH its channels: a notification interleaved in a response stream, and the MCP spec's standalone GET SSE stream, opened via the new McpTransport::start_notification_stream hook that manager.rs calls after notifications/initialized. The evidence is the end-to-end proof through McpManager; it fails on the pre-fix tree with refresh_signalled_tools() returning []."
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
  - id: c4
    text: "Every transport that owns a server-to-client channel observes tools/list_changed, and none of them can be resurrected after close"
    state: met
    evidence: "test:crates/wcore-mcp/tests/list_changed_non_stdio_transports.rs::a_closed_streamable_http_server_is_never_refreshed_again"
    owner: core
    note: "The class criterion c1's original evidence could not carry: a source lint over main.rs counts registration call sites and says nothing about whether a transport can hear a notification. Positive arms: sse_transport_observes_tools_list_changed, streamable_http_observes_a_notification_inside_a_response_stream, streamable_http_observes_the_standalone_notification_stream. Negative controls that pass in BOTH arms: sse_transport_ignores_every_other_id_less_frame (a resources notification, an unrelated notification and a plain response must not raise the flag) and a_server_that_refuses_the_standalone_stream_still_works (a spec-legal 405 on the standalone GET must not break connect or requests). The resurrection half is the hazard named alongside the defect: StreamableHttpTransport inherited is_alive() -> true and its close() could not make it false, so implementing take_tools_changed there without also fixing is_alive would have let the manager re-register an operator-removed server's tools on its next list_changed. This test runs the OPEN arm first as a control, so the closed arm's empty result cannot be an artefact of nothing being signalled."
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
