---
issue: 1175
repo: FerroxLabs/wayland
kind: defect
title: "A runtime-added MCP server's tools/list_changed is ignored for the life of the session"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be"
    state: not-met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "The outcome chosen is 'honoured', not 'say plainly it will not be'. A source lint over main.rs and tui/engine_bridge.rs asserting the register_runtime_server call count and the forget_runtime_server rollback. All three paths are wired: main.rs:3887 (deferred config), main.rs:5702 (AddMcpServer), tui/engine_bridge.rs:3043 (/mcp add), with :3117 forgetting on rollback. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: Evidence resolves and is non-vacuous, but the criterion as written does not hold for two of the three MCP transports. RESOLVED+RAN: test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh exists at main.rs:11122; `cargo test -p wcore-cli --bins` => 'test tests::every_runtime_mcp_add_joins_the_catalog_refresh ... ok'. NOT VACUOUS: in a throwaway copy I replaced main.rs:5821 `refresh.register_runtime_server(&mgr_arc, &single_configs);` with `let _ = &refresh; // MUTANT` (verified the mutation landed on the code line, not a comment) and the lint went red: 'assertion `left == right` failed ... left: 1, right: 2'. CLASS: I enumerated every production McpManager construction site across crates/*/src (connect_all_with_policy*): bootstrap.rs:1843 (boot), plugins/mcp_delivery.rs:167 (boot, plugin, called only from bootstrap.rs:1889), main.rs:5452 (#551 deferred -> registers via integrate_deferred_mcp at main.rs:3974), main.rs:5742 (AddMcpServer -> registers at :5821), tui/engine_bridge.rs:3038 (/mcp add -> registers at :3141, rollback forgets at :3215). All three runtime paths register, and the mechanism is live: bootstrap.rs:3534 installs the refresh unconditionally and engine.rs:12855 calls refresh_mcp_catalog() at the top of every turn. WHY IT STILL FAILS: only StdioTransport ever detects the notification. take_tools_changed() defaults to false in transport/mod.rs:35 and is overridden ONLY in stdio.rs:961. SseTransport's listener drops every id-less frame (`&& let Some(id) = response.id`, sse.rs:292) and the file contains zero occurrences of tools_changed; StreamableHttpTransport overrides neither take_tools_changed nor is_alive and has no server-initiated stream at all. So refresh_signalled_tools (manager.rs:711) can never fire for an SSE or Streamable-HTTP server: a server attached at runtime over a URL transport still has its tools/list_changed ignored for the life of the session, and nothing says so — failing BOTH of the criterion's two disjuncts. SECONDARY (not by itself fatal): the evidence is a source lint over include_str! text, i.e. a regression guard not a class guard — a FOURTH bare runtime-add path in main.rs would leave the count at 2 and pass. I closed that by hand-enumerating call sites; the stated evidence does not."
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
