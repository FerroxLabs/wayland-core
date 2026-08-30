---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: closed
last_verified_commit: 16658a2f
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: met
    evidence: "symbol:crates/wcore-cli/src/main.rs::withdraw_runtime_mcp_manager"
    owner: core
    note: "Fixed as a shape, not as two missing lines. Dropping the manager from dynamic_managers and withdrawing it from McpCatalogRefresh are now ONE function, and it is the only place in main.rs that does either; both removal paths (remove_runtime_mcp_server, the RemoveMcpServer handler, and teardown_runtime_mcp_for_replace, the AddMcpServer{replace:true} half) call it. Nothing in the fix consults is_alive(), so the withdrawal holds for a transport that reports alive after close -- the trait default. Distinct from the wave-2 MCP lane (FerroxLabs/wayland#1213, merged into integ/f13 as 431273f3a): that lane gave StreamableHttpTransport take_tools_changed and a close-aware is_alive, i.e. it made the LIVENESS guard real; it added no forget_runtime_server call to main.rs (origin/integ/f13:main.rs contains exactly one occurrence of the string, the source lint's own literal). This lane removes the dependence on that guard."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_removed_runtime_server_is_not_refreshed_even_when_its_transport_reports_alive"
    owner: core
    note: "Two arms on the same fixture, connected through the production integrate_deferred_mcp path so the server is registered with the refresh exactly as a live session registers it. Arm A withdraws and asserts refresh.apply() returns empty and the announced tool never enters the registry; arm B does not withdraw and asserts the same announcement DOES bring the tool back, so arm A is the withdrawal working rather than a fixture that announced nothing. The test opens by asserting the fixture transport's is_alive() is true (it takes the trait default), so the pass cannot be attributed to the liveness guard."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "The lint counts the SHAPE rather than a list of sites: main.rs must contain exactly one manager-drop predicate and exactly one forget_runtime_server call, and the withdrawal helper's own body must contain both -- so a second removal path that drops a manager inline fails the count, and splitting the helper into two statements fails the body check. Adding the missing call at each site twice would have passed a site-counting lint and left the next removal path free to omit it. Both needles are built with concat! so this file does not match its own literals; the pre-existing TUI assertion was switched to the same fragment for that reason."
---

The json-stream `RemoveMcpServer` path closes the transport and drops the
manager from `dynamic_managers`, but `McpCatalogRefresh` keeps the `Arc` it took
in `register_runtime_server`, so the removed server stays registered for the
life of the session.

Not currently exploitable: `McpManager::refresh_signalled_tools` skips a
transport reporting `is_alive() == false`, and all three transports now do so
after `close()`. The hazard is that this liveness flag, not the withdrawal, is
the only thing standing between an operator's removal and the server
re-registering its tools on its next `tools/list_changed`.
