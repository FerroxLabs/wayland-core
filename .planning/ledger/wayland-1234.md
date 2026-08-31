---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: closed
last_verified_commit: 4e4f9d53f
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: met
    evidence: "symbol:crates/wcore-cli/src/main.rs::RuntimeMcpManagers"
    owner: core
    note: "MET, and as a TYPE rather than two missing lines. `dynamic_managers` is `RuntimeMcpManagers`, whose Vec is private to `mod runtime_mcp_managers` and whose only removal is `withdraw`, which drops the manager AND calls `forget_runtime_server` as one operation. Both host-protocol removal paths and the TUI `/mcp remove` path go through it, so withdrawal cannot depend on what a transport reports about liveness. MERGE NOTE, because it changes behaviour and was a real conflict: integ/f13 had its own #1213 c4 fix on the TUI path that withdrew only AFTER `close_server` succeeded, leaving the `CleanupUnverified` arm un-withdrawn. This tree takes the fused version, which withdraws on that arm too -- and that arm is precisely the one whose transport may still be alive and announcing `tools/list_changed`, i.e. #1234's own resurrection shape."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_removed_runtime_server_is_not_refreshed_even_when_its_transport_reports_alive"
    owner: core
    note: "MET. Two arms on one fixture through the production `integrate_deferred_mcp` path: arm A withdraws and asserts `refresh.apply()` returns empty and the announced tool never enters the registry; arm B does not withdraw and asserts the same announcement DOES bring it back. Arm B is what stops arm A passing vacuously, and neither relies on liveness."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "MET, with BOTH lints kept rather than one replacing the other. integ/f13's tree-walking guard (`every_runtime_mcp_withdrawal_leaves_the_catalog_refresh`) reads every file under wcore-cli/src and grades per FUNCTION, which the peer lane's does not; its needle set was moved onto the fused spellings (`_managers.withdraw(`, `retire_runtime_mcp_server(`, `forget_runtime_server(`) because the helper it used to count no longer exists. Its pinned (file, fn) control set is unchanged, so a site going missing still reddens."

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
