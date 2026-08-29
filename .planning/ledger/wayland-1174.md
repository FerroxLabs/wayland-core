---
issue: 1174
repo: FerroxLabs/wayland
title: "Under defer_config_mcp the engine gets no catalog refresh at all: mid-session tools/list_changed is invisible for every server"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A session running with defer_config_mcp honours tools/list_changed for its config-declared servers"
    state: not-met
    owner: core
    note: "today mcp_refresh_configs stays empty, is_empty() is true, and set_mcp_catalog_refresh returns early so no refresher is installed"
  - id: c2
    text: "A test drives the deferred-config path and asserts a late-registered tool becomes callable"
    state: not-met
    owner: core
    note: "the issue states this test fails today"
  - id: c3
    text: "The per-tool allowlist from #998 is still honoured on the refresh path that defer_config_mcp gains"
    state: not-met
    owner: core
    note: "#998 c4 is open on exactly this: the enforcement is real and bypassed by the mode Desktop runs in"
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
