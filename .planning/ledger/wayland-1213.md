---
issue: 1213
repo: FerroxLabs/wayland
kind: defect
title: "notifications/tools/list_changed is silently ignored for every SSE and Streamable-HTTP MCP server, at boot and at runtime"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "An SSE server's notifications/tools/list_changed reaches McpManager::refresh_signalled_tools, or the product says plainly at attach time that this transport will not honour it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D31, found while verifying wayland#1175). Nothing has been done. The measured finding, verbatim: notifications/tools/list_changed is silently ignored for every SSE and Streamable-HTTP MCP server, at boot and at runtime alike. McpTransport::take_tools_changed() defaults to `false` (crates/wcore-mcp/src/transport/mod.rs:35) and is overridden ONLY by StdioTransport (crates/wcore-mcp/src/transport/stdio.rs:961). SseTransport has no tools_changed state at all and its listener drops every id-less frame — `&& let Some(id) = response.id` at crates/wcore-mcp/src/transport/sse.rs:292 — which is precisely the shape of a JSON-RPC notification. StreamableHttpTransport overrides neither take_tools_changed nor is_alive and consumes text/event-stream only as the framing of a reply to its own request (crates/wcore-mcp/src/transport/streamable_http.rs:197), so it has no channel for a server-initiated notification. McpManager::refresh_signalled_tools (crates/wcore-mcp/src/manager.rs:711) therefore skips these servers unconditionally, and the whole McpCatalogRefresh machinery is a no-op for them."
  - id: c2
    text: "The same disjunct is satisfied for StreamableHttpTransport"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D31). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "SseTransport's listener no longer drops id-less frames unconditionally; a test asserts a notification frame is dispatched"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D31). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "If take_tools_changed is implemented for StreamableHttpTransport, is_alive/close are fixed in the same change and RemoveMcpServer withdraws the entry, so an operator-removed server cannot be resurrected by a later list_changed"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D31). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

notifications/tools/list_changed is silently ignored for every SSE and Streamable-HTTP MCP server, at boot and at runtime alike. McpTransport::take_tools_changed() defaults to `false` (crates/wcore-mcp/src/transport/mod.rs:35) and is overridden ONLY by StdioTransport (crates/wcore-mcp/src/transport/stdio.rs:961). SseTransport has no tools_changed state at all and its listener drops every id-less frame — `&& let Some(id) = response.id` at crates/wcore-mcp/src/transport/sse.rs:292 — which is precisely the shape of a JSON-RPC notification. StreamableHttpTransport overrides neither take_tools_changed nor is_alive and consumes text/event-stream only as the framing of a reply to its own request (crates/wcore-mcp/src/transport/streamable_http.rs:197), so it has no channel for a server-initiated notification. McpManager::refresh_signalled_tools (crates/wcore-mcp/src/manager.rs:711) therefore skips these servers unconditionally, and the whole McpCatalogRefresh machinery is a no-op for them.

**Where.** crates/wcore-mcp/src/transport/mod.rs:35, crates/wcore-mcp/src/transport/sse.rs:290-294 and :450, crates/wcore-mcp/src/transport/streamable_http.rs:325-370, consumed at crates/wcore-mcp/src/manager.rs:711

**Why it matters.** It is #1175's own reported symptom, unfixed, for the transports most likely to be used with the feature the ticket calls documented: `/mcp add` with a URL. A hosted MCP server attached mid-session announces a new tool, the announcement is discarded, and the tool stays uncallable for the rest of the session with no warning and no way for the user to tell — the exact wording of the ticket. Nothing in the product says non-stdio servers are excluded, so #1175's acceptance ('honoured, or the product says plainly that it will not be') fails on both disjuncts for two of three transports. Secondary hazard: StreamableHttpTransport inherits is_alive() -> true and its close() cannot make it false, so if take_tools_changed is ever implemented there without also fixing is_alive, the RemoveMcpServer path's missing forget_runtime_server call (crates/wcore-cli/src/main.rs:3726, which never withdraws the entry) turns into a live resurrection bug: an operator-removed server's tools would be re-registered into the live registry on its next list_changed.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
