---
issue: 1175
repo: FerroxLabs/wayland
kind: defect
title: "A runtime-added MCP server's tools/list_changed is ignored for the life of the session"
status: open
last_verified_commit: a1488faf5
criteria:
  - id: c1
    text: "An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be"
    state: met
    evidence: "test:crates/wcore-mcp/tests/url_transport_tools_changed.rs::an_sse_server_has_its_tools_list_changed_honoured"
    owner: core
    note: "RE-GRADED 2026-08-29, class guard CORRECTED 2026-08-30. The wiring half was and is correct - every runtime-add path registers with McpCatalogRefresh, and every_runtime_mcp_add_joins_the_catalog_refresh (wcore-cli/src/main.rs) still guards it. But the criterion FAILED for two of MCP's THREE transports, and failed BOTH of its disjuncts for them: McpTransport::take_tools_changed defaults to false (transport/mod.rs) and was overridden ONLY by StdioTransport, so refresh_signalled_tools skipped every SSE and Streamable HTTP server unconditionally and nothing said so. Fixed for all three: SseTransport detects the notification on its event stream (and its listener now drains the buffer carried over from the handshake, without which a frame batched into the endpoint chunk was dropped outright); StreamableHttpTransport detects it on BOTH channels the spec gives a server - interleaved in a POST response stream, and the standalone GET event stream it now opens - and classifies method-bearing frames as server messages, which also stops a notification being handed back as the reply. It also gained a real is_alive, without which refresh_signalled_tools could re-register the tools of a server the operator had removed. Streamable-HTTP arms: a_streamable_http_notification_in_a_response_stream_is_honoured_and_not_mistaken_for_the_reply and a_streamable_http_server_has_its_standalone_stream_listened_to, each with a negative control. CLASS GUARD - CONCEDED to the verifier and rewritten. transport::tests::every_transport_decides_take_tools_changed_for_itself was described as grading the class, and it did not: it `include_str!`d a HARDCODED list of the three transport files, so a FOURTH transport would simply not be in the list, would inherit the silent false default, and the lint would stay green - the same defect in a new file. The set is now DISCOVERED by walking crates/ at test time for every file carrying a column-zero `impl McpTransport for`, excluding target/ and tests/. The indentation is the discriminator and it was measured, not assumed: `grep -rn \"^impl McpTransport for\" crates/*/src` returns exactly stdio.rs, sse.rs and streamable_http.rs, because all 20-odd mocks live inside inline `#[cfg(test)] mod tests` and are therefore indented - and a mock legitimately may inherit the default, it observes no server-initiated stream. The walk carries a POSITIVE CONTROL asserting those three known files are among what it found, so a broken walk fails loudly instead of grading an empty set. Considered and rejected: deleting the trait default outright, which would be a compile-time class guard but forces the method onto 25+ legitimate test mocks across five crates. RED ARM 2026-08-30: adding crates/wcore-mcp/src/transport/websocket.rs with an `impl McpTransport for WebsocketTransport` that overrides nothing now fails with 'websocket.rs implements McpTransport but inherits the `false` default for take_tools_changed'; the previous hardcoded lint could not have seen that file at all."
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
