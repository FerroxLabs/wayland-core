---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: closed
last_verified_commit: 56b54a06e
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: met
    evidence: "symbol:crates/wcore-cli/src/main.rs::withdraw_runtime_mcp_from_refresh"
    owner: core
    note: "MET at HEAD -- and the previous note was STALE, which is why this row was re-graded rather than inherited. It claimed the fix was a TYPE (`RuntimeMcpManagers`, whose only removal also withdrew). That type does not exist in this tree: it landed in 780dbd722 and was dropped by a later merge that took integ/f13's side of main.rs. What is here instead is `withdraw_runtime_mcp_from_refresh(engine, name)` called from every withdrawal path, and it does satisfy c1 AS WRITTEN: `remove_runtime_mcp_server` withdraws on the ordinary `Removed` arm AND on the `CleanupUnverified` arm -- the arm whose transport may still be alive and announcing `tools/list_changed`, i.e. #1234's own resurrection shape. Graded by behaviour below, not by the shape of the helper."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_removed_runtime_server_is_not_resurrected_by_a_later_list_changed"
    owner: core
    note: "MET, with the evidence pointer CORRECTED: the test the old note named (`a_removed_runtime_server_is_not_refreshed_even_when_its_transport_reports_alive`) does not exist in this tree -- it was lost with the type in the same merge. The property is guarded by two tests that are STRONGER, because both drive the production `remove_runtime_mcp_server` command handler with a real `RemoveMcpServerCommand` rather than calling a helper: `a_removed_runtime_server_is_not_resurrected_by_a_later_list_changed` (Removed arm) and `an_unverified_removal_still_withdraws_so_the_server_cannot_resurrect` (CleanupUnverified arm, with a lifecycle assertion pinning WHICH arm ran). THE LIVENESS PRECONDITION IS NOW MEASURED, not asserted in a doc comment: this lane added `assert!(SharedTransport(Arc::new(GrowingTestTransport::new(&[]))).is_alive())` to the first test -- neither fixture overrides `is_alive`, so both take the trait default `true` and the refresh's dead-transport skip cannot be what the test is grading. TWO RED ARMS RUN HERE. (1) Deleted the Removed-arm `withdraw_runtime_mcp_from_refresh(engine, &command.name);`, `touch`, `cargo check -p wcore-cli --tests` RC=0 -> `a removed server must not be refreshed at all; refreshed ['warehouse']`. (2) Restored, then deleted the CleanupUnverified-arm call, `touch`, RC=0 -> `wayland#1234: a server removed on the CleanupUnverified arm was still polled by McpCatalogRefresh. Refreshed: ['warehouse']`. Restored and green after each."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_add_joins_the_catalog_refresh"
    owner: core
    note: "MET. `every_runtime_mcp_withdrawal_leaves_the_catalog_refresh` derives its file set from `wcore_cli_production_sources()` (every .rs under wcore-cli/src, cfg(test) stripped), grades per FUNCTION, and pins the exact (file, fn) set -- main.rs::remove_runtime_mcp_server, main.rs::teardown_runtime_mcp_for_replace, tui/engine_bridge.rs::connect_and_register_mcp, tui/engine_bridge.rs::remove_tui_runtime_mcp -- so a site going missing drops a pair and reddens. Its needles match the spellings HEAD actually uses. LIMIT, recorded rather than implied away, and MEASURED in red arm (1) above: the lint is per-FUNCTION, so deleting the withdrawal on ONE ARM of `remove_runtime_mcp_server` left the lint GREEN (the other arm still carries the call). The source lint cannot see a per-arm omission; only the two behavioural tests in c2 can, and they did. Both are kept for that reason."

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
