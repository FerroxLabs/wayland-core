# wayland-ijfw

Anchor plugin for the wcore plugin architecture. Exercises every
`register_*` surface on `PluginContext` — tools, hooks, agents,
skills, rules, MCP server — through the `wcore-plugin-api` mirror
types.

REV-2 audit F2 invariant: this crate must NOT depend on `wcore-agent`,
`wcore-tools`, `wcore-mcp`, `wcore-skills`, `wcore-memory`,
`wcore-config`, `wcore-providers`, or `wcore-compact`. The capability
surface flows through api-crate-local mirror types
(`BundledSkillSpec`, `AgentManifest`, `RuleSpec`, `McpServerSpec`).

## Committed IJFW snapshot

`snapshots/ijfw-source/` is a **committed, read-only copy** of the
pinned IJFW project tree (originally `~/dev/ijfw/`, IJFW v1.3.x). The
files referenced by the plugin's `include_str!` calls live entirely in
the repository so that clean CI checkouts can compile without any
external paths or symlinks. The directory contains only the markdown
files the plugin actually embeds — 3 agents, 2 rules, 22 skill bodies.

Updating the snapshot requires explicit re-pinning: copy the new
upstream file into the corresponding `snapshots/ijfw-source/...`
path and re-run `cargo check -p wayland-ijfw`. The crate fails to
build if any referenced file disappears, which surfaces drift at
the next compile.
