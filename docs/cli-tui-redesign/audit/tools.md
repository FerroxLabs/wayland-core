# Audit — TOOLS & tool-call rendering

Slice: built-in tools, tool-call cards, MCP surface, Spawn. Engine = `wcore-core` @ `feat/v0.6.1-hardening`.

## The real built-in tool set

Registration happens in `crates/wcore-agent/src/bootstrap.rs` (not the registry —
`registry.rs` is a generic container). Default-on registrations:

| Tool name (LLM-visible) | Source | Default? |
|---|---|---|
| `Read` | `wcore-tools/src/read.rs:38` | always |
| `Write` | `wcore-tools/src/write.rs:37` | always |
| `Edit` | `wcore-tools/src/edit.rs:36` | always |
| `Bash` | `wcore-tools/src/bash.rs:152` | always |
| `Grep` | `wcore-tools/src/grep.rs:16` | always |
| `Glob` | `wcore-tools/src/glob.rs:19` | always |
| `Git` | `wcore-tools/src/git.rs:103` | always (`bootstrap.rs:317`) |
| `RepoMap` | `wcore-tools/src/repomap.rs:51` | on (`RepoMapToolConfig` default `true`, `tools.rs:30`) |
| `Skill` | `wcore-agent/src/skill_tool.rs:99` | always (`bootstrap.rs:479`) |
| `Spawn` | `wcore-agent/src/spawn_tool.rs:61` | always (`bootstrap.rs:489`) |
| `EnterPlanMode` | `wcore-agent/src/plan/tools.rs:36` | on (`PlanConfig` default `true`, `plan.rs:12`) |
| `ExitPlanMode` | `wcore-agent/src/plan/tools.rs:117` | on (same gate) |
| `ToolSearch` | `wcore-tools/src/tool_search.rs:32` | always (`bootstrap.rs:594`) |
| `Script` | `wcore-tools/src/script.rs:290` | **off** by default (`ScriptToolConfig` default `false`, `tools.rs:24`) |

**Real default count = 13** (Read, Write, Edit, Bash, Grep, Glob, Git, RepoMap,
Skill, Spawn, EnterPlanMode, ExitPlanMode, ToolSearch). 14 if `Script` is
enabled. MCP tools are *added on top* of that count, not part of it
(`bootstrap.rs:333` registers them after `builtin_names` is snapshotted).

## Findings

| # | Mockup element | Engine reality (file:line) | Verdict | Fix needed |
|---|---|---|---|---|
| 1 | Tools rail says **"12"** | Default builtins = **13**, not 12. `Write` is conspicuously absent from the count's mental model. | WRONG | Change `12` → `13` (builtin-only) or, if the "12" was meant to include MCP, label it as builtin count = 13 and show MCP separately. |
| 2 | Rail lists Read, Edit, Bash, Grep, Glob then "+7 more" | The 5 shown are all real. But "+7 more" implies 5+7=12. Real builtins beyond those 5 = **8** (Write, Git, RepoMap, Skill, Spawn, EnterPlanMode, ExitPlanMode, ToolSearch). | RENAME | "+8 more" (builtins). `Write` is a glaring omission from the top-5 — it is a core always-on tool and arguably belongs in the visible list over `Glob`. |
| 3 | "· 2 MCP" appended to the tool list | MCP tools are real and registered (`wcore-mcp/src/tool_proxy.rs:131` `register_mcp_tools`). Count is dynamic — depends entirely on configured `mcp.servers`; could be 0. Naming: a bare tool name if unique, else `mcp__{server}__{tool}` on collision (`tool_proxy.rs:15-16,152`). | ASPIRATIONAL-OK | Capability is real (MCP client). "2" is a fine illustrative number; just ensure copy treats it as session-dependent, not fixed. |
| 4 | Card: `Read src/auth/anthropic.rs → "124 lines"` chip | `ReadTool` returns the **full file content as numbered lines** (`read.rs:249-255`, `cat -n` style), not a line-count summary. There is no "N lines" result string. The protocol `ToolResult` event (`events.rs:49-58`) carries `output: String` + optional opaque `metadata: Value` — a host *could* derive a line count from `output`, but the engine does not emit one. | RENAME / ASPIRATIONAL-OK | The chip is a host-side UI summary, legitimate to show — but label honestly. "124 lines" must be computed by the TUI from the returned content, not presented as an engine field. Acceptable as a derived chip; flag in spec that it's host-derived. |
| 5 | Card: `Grep "compat" in src/auth/ → "7 hits"` chip | `GrepTool` (ripgrep, `grep.rs:125-158`) returns **matching lines joined** (`rg -n` output, capped at 250 lines), or the literal string `"No matches found"`. No hit count is emitted. | RENAME / ASPIRATIONAL-OK | Same as #4 — "7 hits" must be a host-derived count of returned lines. Note: result is line-capped at 250 (`grep.rs:154`), so a "hits" chip on a capped result would undercount; chip should read e.g. "250+ lines" when capped. `describe()` returns `Grep '{pattern}' in {path}` (`grep.rs:115`) — the arg-summary format in the card matches this well. |
| 6 | Card: `Edit src/auth/anthropic.rs → "+12 −3"` chip | `EditTool` returns exactly `"Edited {file}: replaced {N} occurrence(s)"` (`edit.rs:195-198`, `edit.rs:324`). **No +/− line counts, no diff** are produced by the tool. The engine has no diff-generation in the Edit path. | WRONG | The `+12 −3` chip cannot come from the engine. Either (a) drop the +/− chip and show "replaced 1 occurrence", or (b) keep it but the TUI must compute the diff itself from old vs new file state. The separate full diff block in the mockup has the same issue — Edit emits no structured diff; a TUI diff view is net-new host work. Mark as ASPIRATIONAL if the spec commits to host-side diffing; otherwise WRONG. |
| 7 | Tools panel per-call counts: `Read ×2`, `Grep ×1`, `Edit ×1`, `Bash —` | Per-tool invocation counts are not a first-class engine concept, but every tool call surfaces as a `ToolCall`/`ToolResult` protocol event pair with `tool_name` (`events.rs:44-58`). A host can trivially tally these. | ASPIRATIONAL-OK | Grounded — counts are derivable from the `ToolCall` event stream. No engine change needed; document as host-side aggregation. |
| 8 | Agent Manager implies a `Spawn`-style sub-agent tool | `SpawnTool` exists, name is literally `"Spawn"` (`spawn_tool.rs:61`), backed by `AgentSpawner` (`bootstrap.rs:485-489`). It **is** a registered tool. | ACCURATE | None. The tool is named `Spawn` — if the manager UI labels a button it can say "spawn agent" (lowercase verb) consistently with the tool. |
| 9 | Tool-card glyph + name + arg summary layout | Matches the engine model: `ToolInfo { name, category, args, description }` (`events.rs:482-488`) gives glyph-by-category, name, and arg summary. `ToolCategory` enum = `Info / Edit / Exec / Mcp` (`events.rs:492-497`) — a 4-way glyph/color scheme is well-grounded. | ACCURATE | Optionally drive the glyph color off `ToolCategory` (Info/Edit/Exec/Mcp) — the mockup uses one orange dot for all. |

## Tools the mockup OMITS that exist

- **`Write`** — always-on, core. Absent from the rail's visible 5 *and* arguably mis-counted. Should be visible alongside Read/Edit.
- **`Git`** — first-class typed tool (`git.rs`), ops: status, diff, log, add, commit, checkout, stash, branch. The mockup shows a git *branch* in the status bar but never the `Git` tool; a "refactor auth" session realistically calls it.
- **`RepoMap`** — default-on symbol indexer. The Workspace activity feed even says "repomap indexed 777 files" (mockup line 626), so the *capability* is shown — but `RepoMap` as an invokable tool is missing from the tool list.
- **`Skill`** — the skills system is a registered tool; the mockup's command palette shows slash-commands but never the `Skill` tool surface.
- **`ToolSearch`** — deferred-tool loader (`tool_search.rs`); relevant because several tools are `is_deferred()` (Spawn `spawn_tool.rs:109`, EnterPlanMode/ExitPlanMode `plan/tools.rs:57,137`) and only become callable after a `ToolSearch`. A "+7 more" collapsed list is conceptually a deferred-tool UX — worth naming.
- **`EnterPlanMode` / `ExitPlanMode`** — the mockup *has* a Plan badge and a Plan Review tab, so plan mode is depicted, but the two tools that drive it aren't in the tool inventory.
- **`Script`** — exists but default-off; fine to omit, but if the rail ever shows a full inventory it belongs there with an "off" state.

## Bottom line

Tool *names* the mockup shows (Read, Edit, Bash, Grep, Glob, Spawn) are all
correct verbatim. Two hard errors: the **count "12" is wrong (real default is
13)**, and the **`Edit` `+12 −3` chip + diff block depict output the engine does
not produce** — `Edit` returns only `"Edited …: replaced N occurrence(s)"`. The
`Read` "N lines" and `Grep` "N hits" chips are defensible *only* as
explicitly host-derived summaries, not engine fields. `Write` is the most
glaring omission from the visible tool list.
