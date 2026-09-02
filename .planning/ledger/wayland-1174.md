---
issue: 1174
repo: FerroxLabs/wayland
kind: defect
title: "Under defer_config_mcp the engine gets no catalog refresh at all: mid-session tools/list_changed is invisible for every server"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "A session running with defer_config_mcp honours tools/list_changed for its config-declared servers"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::set_mcp_catalog_refresh"
    owner: core
    note: "The is_empty() early return is gone; the body is unconditional and the reason is documented at engine.rs:5187-5193. Config-declared servers join via main.rs:3887 register_runtime_server inside integrate_deferred_mcp."
  - id: c2
    text: "A test drives the deferred-config path and asserts a late-registered tool becomes callable"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::deferred_config_mcp_still_honours_a_late_tools_list_changed"
    owner: core
    note: "Installs an EMPTY McpCatalogRefresh exactly as bootstrap does under defer_config_mcp, drives the real integrate_deferred_mcp, then asserts the late tool is absent before and callable after. Paired with mcp_dynamic_tools.rs::a_refresh_that_started_empty_serves_the_deferred_config_connect."
  - id: c3
    text: "The per-tool allowlist from #998 is still honoured on the refresh path that defer_config_mcp gains"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::a_deferred_servers_empty_allowlist_survives_the_refresh"
    owner: core
    note: "Structurally enforced too: register_runtime_server (tool_proxy.rs:429) REFUSES a manager whose server_configs map is empty and logs at ERROR, keeping the 'config == None means allow-all' read out of reach. Graded by mcp_dynamic_tools.rs::a_runtime_manager_with_no_config_is_refused."
---

When `defer_config_mcp` is set — the mode the Wayland Desktop host runs in — the
config manager never enters `mcp_managers`, `McpCatalogRefresh::is_empty()` is
true, and `set_mcp_catalog_refresh` returns early. The engine ends up with no
catalog refresh installed at all, so a `notifications/tools/list_changed` from
any server in that session is silently ignored, for every server, for the whole
session.

Honouring `tools/list_changed` mid-session shipped as a feature in v0.13.0. This
says it is absent in precisely the configuration most users run, while the code
and the release note both say it is on, and nothing reports the discrepancy.

Nothing has been built. c3 is carried here because #998's per-tool enforcement
rides on the refresher: whoever installs one under `defer_config_mcp` must carry
the allowlist across it, and #1175's notes warn that these two must land as one
change or the allow-all read goes live on a refresh. Not a security issue as it
stands: no allowlist is dropped, because no refresh happens at all.
