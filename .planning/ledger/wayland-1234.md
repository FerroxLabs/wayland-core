---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: open
last_verified_commit: 4e7da4338
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 while closing FerroxLabs/wayland#1175 c1. forget_runtime_server has exactly one production caller, the TUI /mcp add rollback at crates/wcore-cli/src/tui/engine_bridge.rs:3215; neither remove_runtime_mcp_server nor the replace helper in crates/wcore-cli/src/main.rs calls it. Today it is a leak rather than a live defect only because refresh_signalled_tools skips a transport whose is_alive() is false - which is now the entire protection, since all three transports report tool changes as of #1175."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: not-met
    owner: core
    note: "The is_alive() -> true case is the trait default and the correct answer for a stateless HTTP transport, so the test must not rely on liveness to pass."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: not-met
    owner: core
    note: "main.rs:11145 currently asserts only that tui/engine_bridge.rs contains a forget_runtime_server call."
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
