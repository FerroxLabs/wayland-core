# A-11 — a tool registered mid-session must become callable

Lane `lane/a11-mcp-dynamic`, based on `integ/round2-base` @ `8d69d402`.

## The defect

Corpus row A-11 declares the warehouse as a stdio MCP server with
`deferred = false`. It advertises 41 tools at connect and registers a 42nd,
`inventory_audit_export`, only after the first successful despatch — announcing
it with `notifications/tools/list_changed`, which is precisely what the MCP
`tools.listChanged` capability exists for.

Nothing in the product ever looked at that notification.

`crates/wcore-mcp/src/transport/stdio.rs` (pre-fix, line 643) routed inbound
lines by JSON-RPC `id`; every id-less line fell into a single branch that
logged at `debug!` and dropped it:

```rust
None => {
    // Notification / log line with no id —
    // not a response to any request. Drop it
    // rather than mis-matching it (audit C3).
    debug!(server = %label, line = %trimmed,
           "[mcp] stdio notification ignored (no id)");
}
```

A grep for `list_changed` across the whole workspace returned only
`crates/wcore-mcp/src/server.rs:270`, where our own *server* side advertises
`"tools": {"listChanged": false}`. The client had no subscriber, no re-list,
and no path by which a late tool could reach the registry.

Downstream of that, tool discovery was structurally one-shot:

* `McpServer.tools` was a plain `Vec<McpToolDef>` written once by
  `connect_server` and never again. The manager is shared as
  `Arc<McpManager>` by every tool proxy, so nothing could write to it.
* `McpManager::all_tools()` returned borrows into that `Vec`.
* The live `ToolRegistry` was populated at boot and only re-touched by the
  `/mcp add` live-add seam.

So the tool was invisible in `all_tools()`, absent from the registry, absent
from the outbound `tools[]`, and absent from the `ToolSearch` catalogue — for
the life of the session.

**This is a distinct defect from the earlier MCP repair** (whole-query
substring match / bulk registration never refreshing the catalogue / no
callability signal). That work made a *statically discovered* tool reachable.
Verified against this tree: `register_mcp_tools` does refresh the `ToolSearch`
snapshot, and it is correct — it simply runs once, over a tool list that could
not change.

## The fix

Three seams, dependencies flowing downward only.

1. **`crates/wcore-mcp/src/transport/mod.rs`** — `McpTransport` gains
   `fn take_tools_changed(&self) -> bool { false }`. Only the transport owns
   the inbound stream, so only the transport can observe an id-less
   notification. Take-and-clear, so one signal cannot drive an unbounded
   re-list loop. Transports that do not observe server notifications keep the
   default `false`; today that is SSE and streamable-HTTP.

2. **`crates/wcore-mcp/src/transport/stdio.rs`** — a `tools_changed:
   Arc<AtomicBool>` shared with the reader task. The id-less branch now
   classifies the line via `notified_tools_changed()` and raises the flag for
   `notifications/tools/list_changed` only; everything else is still dropped,
   unchanged.

3. **`crates/wcore-mcp/src/manager.rs`** — `McpServer.tools` becomes
   `RwLock<Vec<McpToolDef>>`, and `McpManager::refresh_signalled_tools()`
   re-issues `tools/list` for each live server that signalled. A server that
   said nothing sends no traffic (one atomic load). A re-list that fails keeps
   the previous catalogue rather than deleting tools the model is mid-way
   through using.

   Consequence: `all_tools()` now returns `Vec<(String, McpToolDef)>` instead
   of borrows — no reference can outlive the read guard. Six call sites
   adjusted; it is not on a per-turn path.

4. **`crates/wcore-mcp/src/tool_proxy.rs`** — `McpCatalogRefresh` bundles the
   managers plus the `builtin_names` / server-config snapshots boot used, so a
   refreshed tool gets the same collision prefix and deferral it would have had
   at connect. `apply()` drops the server's tools from the registry and
   re-registers wholesale (a `list_changed` can REMOVE a tool too, and a stale
   proxy is a call that fails at the far end), reusing
   `register_single_server_tools` so the `ToolSearch` snapshot is refreshed in
   the same pass.

5. **`crates/wcore-agent/src/engine.rs`** — `refresh_mcp_catalog()` runs at the
   TOP of each turn, before the outbound `tools[]` is built, so a tool that
   appeared during turn N is offered on turn N+1. That is the only point in
   the loop where the registry `Arc` is uniquely held (the per-turn executor
   adapter takes a clone and drops it at the end of the iteration), so
   `Arc::get_mut` succeeds there and nowhere later; a leaked clone is warned
   about, not swallowed. On a real change the MCP curation / cap caches are
   invalidated, because they are keyed on an inventory that just moved.

6. **`crates/wcore-agent/src/bootstrap.rs`** — wires the bundle after the
   plugin MCP second pass. Plugin-supplied servers are deliberately not in the
   config map: `translate_mcp_server_spec` sets `deferred: None`, which is
   exactly what a lookup miss already resolves to.

### Deliberately NOT done

*Refresh-and-retry on a call to an unknown tool.* Both observed failures below
are the model declining to call a tool it could plainly see; in neither did it
attempt an unknown name. Adding a retry path would be machinery with no
evidence behind it, and the dispatch site holds only an `Arc<ToolRegistry>`, so
it could not mutate anyway.

## Evidence

### RED arm — unit

Two independent controls, each mutating CODE (not a comment) and each restored
with `touch` afterwards so cargo could not hand back the mutant binary.

* Restore the drop-the-notification branch in `stdio.rs`:
  `a11_tools_list_changed_notification_is_observed_and_take_cleared` FAILED,
  `a11_unrelated_notifications_do_not_raise_tools_changed` still passed
  (1 passed; 1 failed).
* Early-return from `McpManager::refresh_signalled_tools`: all 3 tests in
  `mcp_dynamic_tools.rs` FAILED (0 passed; 3 failed).

Both restored: 2 passed / 3 passed respectively.

### RED arm — live corpus

Built from a clean detached worktree at the lane's base `8d69d402`.

| | |
|---|---|
| binary sha256 | `3663bd6cd8f308180dae9a883646d979bec6e20fd99abd7b7ad4fe69ef01bdb4` |
| A-11 verdict | **FAIL** |
| `A-11.the-warehouse-really-moved` | FAIL |
| `A-11.the-audit-export-was-written` | FAIL, `server_recorded_export: False`, `export_file_present: True` |

The file existed and the warehouse had no record of the tool running — the
model hand-wrote an export. Graded from the warehouse's own audit table, not
from the file.

### GREEN — live corpus, 6 runs

All six graded binary sha256 `a2718e5b5f20ccf51cdf0a1bb29f78447fe348d3d62bdcc397419dc8e23b7d3a`.

| run | verdict | `server_recorded_export` |
|---|---|---|
| 1 | FAIL | False |
| 2 | PASS | True |
| 3 | PASS | True |
| 4 | PASS | True |
| 5 | FAIL | False |
| 6 | PASS | True |

**4/6.** Baseline is 0 and structurally so.

Binary identity was checked by marker grep, with a positive control so a
silently-zero grep could not pass for evidence:

| string | RED | GREEN |
|---|---|---|
| `[mcp] server signalled tools/list_changed` | 0 | 1 |
| `notifications/initialized` (positive control) | 3 | 3 |

(The literal `"notifications/tools/list_changed"` does not appear in either
binary: rustc compiles the fixed-length comparison to inline immediates rather
than a `.rodata` string. The `tools/list_changed` log strings — 13 occurrences
in GREEN, 0 in RED — are the usable marker.)

### Why runs 1 and 5 failed

Not a refresh failure. In both, the captured provider requests show
`inventory_audit_export` reaching the model from the turn after the first
despatch onward (green1: 3 of 11 requests; green5: 5 of 13 — i.e. every
request after it was registered). The warehouse recorded the reserves and
commits, so the MCP path was live. The model wrote `audit-export.json` with
`Write` instead of hydrating and calling the tool.

The tool arrives in the `ToolSearch` deferred catalogue rather than in
`tools[]` — but so do all 41 of its siblings: request `0007` (before the
change) and `0008` (after) both carry **zero** warehouse tools in `tools[]`.
The late tool is on exactly the same footing as the tools the model *did* use
successfully. Nothing in the refresh path treats it differently.

### Adjacent observation, NOT fixed here

`wcore_tools::registry::apply_cold_deferral` unconditionally sets
`deferred = true` for every tool off the hot allowlist, overriding an explicit
per-server `deferred = false` in `config.toml`. A user who writes
`deferred = false` is asking for full schemas eagerly and is silently ignored
in the default profile; the row sets it deliberately for that reason. This
predates and is independent of this lane, it affects every MCP tool equally,
and changing it moves token behaviour for every MCP user — so it is reported,
not fixed here. It is the most likely lever on the remaining 2/6.

## Gate

| check | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo check --workspace --all-targets` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo clippy --target x86_64-pc-windows-gnu -p wcore-mcp -p wcore-agent --all-targets -- -D warnings` | clean |
| `wcore-mcp` + `wcore-tools` | 1675 passed, 0 failed, 3 skipped |
| `wcore-mcp` alone (new tests) | 139 lib + 3 `mcp_dynamic_tools` + 21 other, 0 failed |
| `wcore-agent` | 3385 passed, 1 failed, 7 timed out, 6 skipped |
| `wcore-cli` | 2602 passed, 3 timed out, 20 skipped |

All 11 non-passes reproduce identically on the untouched base worktree at
`8d69d402` and are pre-existing:

* `session_journal_test replay_accepts_read_only_authority_files` — fails on
  base, "read-only replay child failed".
* 7 `wcore-agent` orchestration/workflow/council tests and 3 `wcore-cli` tests
  — all exceed nextest's 60s bound on base as well.

These are named, not waved through: they are outside this lane's change and
belong to whoever owns those suites.

## New tests

* `crates/wcore-mcp/src/transport/stdio.rs`
  * `notification_parse_tests::only_the_tools_list_changed_method_matches`
  * `notification_parse_tests::noise_is_false_not_an_error`
    (both cross-platform — the classifier runs on Windows too)
  * `tests::a11_tools_list_changed_notification_is_observed_and_take_cleared`
  * `tests::a11_unrelated_notifications_do_not_raise_tools_changed`
* `crates/wcore-mcp/tests/mcp_dynamic_tools.rs`
  * `a11_manager_relists_only_the_server_that_signalled` — including that an
    idle poll sends no traffic and a silent server is never re-listed
  * `a11_late_tool_reaches_the_live_tool_registry` — dispatchable AND
    discoverable
  * `a11_a_removed_tool_stops_being_dispatchable`
