---
issue: 1234
repo: FerroxLabs/wayland
kind: defect
title: "RemoveMcpServer never withdraws the server from McpCatalogRefresh"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "RemoveMcpServer withdraws the server from McpCatalogRefresh, so a removed server's manager is no longer polled regardless of what its transport reports about liveness"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::an_unverified_removal_still_withdraws_so_the_server_cannot_resurrect"
    owner: core
    note: "REGRADED 2026-08-31 against integ/f13, correcting a not-met grade this cycle recorded on 2026-08-30. That grade came from a real finding -- merge 3e3cb3820 DID discard commit 780dbd722, whose `RuntimeMcpManagers` newtype is absent from the tree while the commit is still an ancestor of it -- but the conclusion drawn from it did not survive contact with HEAD. `RuntimeMcpManagers` was a STRUCTURAL restatement of a fix that already existed, and what the tree carries now is a LATER and stronger route to the same property. The lesson is the one already recorded as `regrade-before-planning`: a dropped commit is evidence about that commit, not about the criterion. MET AS WRITTEN, on BOTH arms, which is what `regardless of what its transport reports about liveness` requires. The normal arm withdraws after `remove_runtime_declaration`; the CleanupUnverified arm -- the one an earlier tree skipped, and the one MOST likely to have a live transport, since it is reached exactly when `close_server` could not be verified -- withdraws before its early return. Nothing on either path consults `is_alive()`. RED ARM RUN 2026-08-31 on the real instrument, `cargo check -p wcore-cli --tests` RC=0 first so each red is behaviour and not a build failure. Mutation A, drop the normal arm's withdrawal: `a_removed_runtime_server_is_not_resurrected_by_a_later_list_changed` reds. Mutation B, drop the CleanupUnverified arm's: `an_unverified_removal_still_withdraws_so_the_server_cannot_resurrect` reds. Each mutation reds EXACTLY ONE test out of the three, so the two arms are separately graded and neither is riding on the other. Restored, `git status --porcelain` = 0, 3/3 green."
  - id: c2
    text: "A test removes a runtime-added server, has it announce tools/list_changed from a transport whose is_alive() is still true, and asserts the tool does not come back"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_removed_runtime_server_is_not_resurrected_by_a_later_list_changed"
    owner: core
    note: "REGRADED 2026-08-31 against integ/f13, correcting a not-met grade this cycle recorded on 2026-08-30. That grade came from a real finding -- merge 3e3cb3820 DID discard commit 780dbd722, whose `RuntimeMcpManagers` newtype is absent from the tree while the commit is still an ancestor of it -- but the conclusion drawn from it did not survive contact with HEAD. `RuntimeMcpManagers` was a STRUCTURAL restatement of a fix that already existed, and what the tree carries now is a LATER and stronger route to the same property. The lesson is the one already recorded as `regrade-before-planning`: a dropped commit is evidence about that commit, not about the criterion. MET AS WRITTEN, and it drives the PRODUCTION handler rather than the withdrawal helper: it calls `remove_runtime_mcp_server` with a real `RemoveMcpServerCommand`, then has the removed server announce a NEW tool and asserts `refresh.apply` returns empty and neither the new tool nor the old ones are in `engine.tools()`. The `is_alive() is still true` clause is satisfied structurally, not by luck: the fixture is `SharedTransport`, which does not override `is_alive`, so it takes the `McpTransport` trait DEFAULT -- the `true` the issue names as the reason this is a resurrection bug rather than a leak. RED ARM: mutation A above."
  - id: c3
    text: "The source lint in crates/wcore-cli/src/main.rs covers the removal path's forget_runtime_server call, not only the TUI rollback"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_runtime_mcp_withdrawal_leaves_the_catalog_refresh"
    owner: core
    note: "REGRADED 2026-08-31 against integ/f13, correcting a not-met grade this cycle recorded on 2026-08-30. That grade came from a real finding -- merge 3e3cb3820 DID discard commit 780dbd722, whose `RuntimeMcpManagers` newtype is absent from the tree while the commit is still an ancestor of it -- but the conclusion drawn from it did not survive contact with HEAD. `RuntimeMcpManagers` was a STRUCTURAL restatement of a fix that already existed, and what the tree carries now is a LATER and stronger route to the same property. The lesson is the one already recorded as `regrade-before-planning`: a dropped commit is evidence about that commit, not about the criterion. MET, and by a lint materially stronger than the one 780dbd722 was written to escape -- which is why re-landing the newtype is NOT owed. That commit's argument was that the guard counted a literal spelling of the retain predicate, so a removal spelled `&command.name` read the same before and after the fix. THAT LINT NO LONGER EXISTS. The one at HEAD walks every `wcore_cli_production_sources()` file, so a withdrawal path in a brand-new file is graded the day it is written; it fails CLOSED on the exact `(file, fn)` SET rather than a count, so a rename that hides a site drops a pair and reds instead of passing quietly; and its one exemption is named in source with its reason. It records its own residual honestly: the DEFECT needles are still a spelling set. RED ARM RUN 2026-08-31, on a mutation that COMPILES (check RC=0, unlike a first attempt that did not and therefore proved nothing): rename `withdraw_runtime_mcp_from_refresh` throughout -- the shape a future author spelling the withdrawal a third way would produce -- and the lint reds naming `fn teardown_runtime_mcp_for_replace`, which is the REPLACE HELPER the issue body names as the second uncovered site. Restored, tree clean, green."

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
