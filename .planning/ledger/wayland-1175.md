---
issue: 1175
repo: FerroxLabs/wayland
title: "A runtime-added MCP server's tools/list_changed is ignored for the life of the session"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be"
    state: not-met
    owner: core
    note: "the issue accepts either outcome; what must stop is the silent opt-out with no warning and no way for the user to tell"
  - id: c2
    text: "A test adds a server at runtime, has it announce a new tool, and asserts that tool becomes callable"
    state: not-met
    owner: core
    note: "the issue states this test fails today"
  - id: c3
    text: "A manager created after boot can be registered with McpCatalogRefresh"
    state: not-met
    owner: core
    note: "its fields are plain Vec/HashMap behind an Arc with &self methods only, so there is no post-construction registration path at all"
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
