---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: closed
last_verified_commit: PENDING
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: met
    evidence: "symbol:crates/wcore-cli/src/main.rs::RuntimeMcpManagers"
    owner: core
    note: "Fixed as a TYPE, not as two missing lines and not as a source lint. dynamic_managers is no longer a Vec<Arc<McpManager>>: it is RuntimeMcpManagers, whose Vec is private to `mod runtime_mcp_managers` and whose only removal is `withdraw`, which drops the manager from the live set AND calls forget_runtime_server as one operation. Both removal paths (remove_runtime_mcp_server, the RemoveMcpServer handler; teardown_runtime_mcp_for_replace, the AddMcpServer{replace:true} half) go through it because there is nothing else to go through -- a drop without a withdrawal is a compile error, not a lint miss. The first attempt fused the two into one helper and guarded the helper with a needle counting the literal `!(manager.hosts_server(name)`; that needle is undecidable over an open alphabet and was measurably vacuous (the historical second removal site spelled the name `&command.name`, so the count read 1 at the pre-fix parent too). Nothing in the fix consults is_alive(), so the withdrawal holds for a transport that reports alive after close -- the trait default. Distinct from the wave-2 MCP lane (FerroxLabs/wayland#1213, merged into integ/f13 as 431273f3a): that lane gave StreamableHttpTransport take_tools_changed and a close-aware is_alive, i.e. it made the LIVENESS guard real; it added no forget_runtime_server call to main.rs. This lane removes the dependence on that guard."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_removed_runtime_server_is_not_refreshed_even_when_its_transport_reports_alive"
    owner: core
    note: "Two arms on the same fixture, connected through the production integrate_deferred_mcp path so the server is registered with the refresh exactly as a live session registers it. Arm A withdraws and asserts refresh.apply() returns empty and the announced tool never enters the registry; arm B does not withdraw and asserts the same announcement DOES bring the tool back, so arm A is the withdrawal working rather than a fixture that announced nothing. The test opens by asserting the fixture transport's is_alive() is true (it takes the trait default), so the pass cannot be attributed to the liveness guard. A second test, the_remove_mcp_server_handler_withdraws_from_the_catalog_refresh, drives the PRODUCTION remove_runtime_mcp_server handler end to end on the same fixture rather than calling the withdrawal directly, which is what previously left the handler bound to the helper only by a source lint. RED ARM: deleting the forget_runtime_server call from RuntimeMcpManagers::withdraw compiles (cargo check -p wcore-cli --all-targets exit 0) and reddens BOTH tests on `a removed server must not be polled at all`."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "The criterion's text is met, but the lint that meets it was rebuilt, because the first one satisfied the words and not the property. It counted the literal `!(manager.hosts_server(name)` -- a needle over an OPEN alphabet: the same drop spelled `server_id`, or written with remove/swap_remove/drain/truncate/clear, matches nothing, and the historical second removal site was spelled `&command.name` and was invisible to it. The predicate is now inverted rather than extended. Because the Vec is private to `mod runtime_mcp_managers`, no caller anywhere in main.rs can drop a manager at all -- the compiler decides that, not a string search -- so the only remaining way to reopen #1234 is a second mutator added INSIDE that module. That, and only that, is what the lint counts, as a CLOSED SET of the module's method names ([admit, into_iter, is_empty, iter, len, new, withdraw]) over the module's own delimited body: any method added, renamed or removed fails it and forces the decision to be taken deliberately. It also still asserts forget_runtime_server has exactly one call site in main.rs and that the call sits inside withdraw's body. RED ARM: adding `pub(crate) fn evict(&mut self, name: &str) { self.live.retain(|m| !m.hosts_server(name)); }` to the module compiles (cargo check -p wcore-cli --all-targets exit 0) and reddens this test on the method-set assertion. SEPARATE ARM, for the class rather than the lint: the verifier's own injection -- a second removal spelled `dynamic_managers.retain(|manager| !(manager.hosts_server(server_id) || manager.health().contains_key(server_id)))` -- no longer COMPILES (E0616, private field `live`), which is the point: the defect is not caught, it is inexpressible."

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
