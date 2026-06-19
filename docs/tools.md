# Built-in Tools

The agent has 7 built-in tools. The LLM automatically selects and invokes them based on the task.

| Tool | Function | Concurrent |
|------|----------|------------|
| **Read** | Read file contents (with line numbers) | Yes |
| **Write** | Write files (auto-creates directories) | No |
| **Edit** | Precise string replacement | No |
| **Bash** | Execute shell commands | No |
| **Grep** | Regex search file contents (via ripgrep) | Yes |
| **Glob** | Find files by pattern matching | Yes |
| **Spawn** | Spawn sub-agents for parallel tasks | No |
| **ToolSearch** | Load schemas for deferred tools | Yes |

---

## Read

Read file contents with line numbers, similar to `cat -n`.

- Supports `offset` and `limit` parameters for reading file slices
- Auto-detects binary files
- Output format: line-numbered text

## Write

Write content to a file atomically.

- Atomic write: writes to a temp file first, then renames
- Auto-creates parent directories

## Edit

Find and replace exact strings in a file.

- Matches `old_string` exactly and replaces with `new_string`
- Requires a unique match by default; errors on multiple matches
- Use `replace_all` to replace all occurrences

## Bash

Execute a shell command and return the result.

- Default timeout: 120 seconds, max 600 seconds
- Returns exit code, stdout, and stderr
- Interpreter: `sh -c` on Unix, `cmd /C` on Windows. **Windows override:** set
  `WAYLAND_BASH_SHELL=powershell` to run commands through Windows PowerShell
  (`powershell -NoProfile -Command`), or `=pwsh` for PowerShell 7+. Affects the
  Bash tool only — hook, MCP, and skill shells are unchanged.

## Grep

Search file contents with regular expressions.

- Uses `rg` (ripgrep) when available, falls back to `grep -rn`
- Supports glob filtering and case-insensitive search
- Results limited to 250 lines

## Glob

Find files matching a glob pattern.

- Standard glob patterns (e.g., `**/*.rs`)
- Results sorted by modification time (newest first)
- Returns up to 100 files

## Spawn

See [Sub-Agent Spawning](advanced.md#sub-agent-spawning) in the Advanced Features guide.

## ToolSearch

Load full schemas for deferred tools so the LLM can invoke them. Deferred tools (from MCP servers with `deferred = true`) are registered by name only — their parameter schemas are not loaded until the LLM calls ToolSearch.

- Query by exact name: `"select:Read,Edit,Grep"`
- Keyword search: `"slack send"` returns best matches
- Returns up to 5 results by default

---

## How It Works

```
User input → Build request (system prompt + history + tool definitions)
           → Stream LLM API response
           → Output text to stdout in real-time
           → If LLM returns tool_use → confirm → execute → send result back
           → Loop until LLM stops calling tools
           → Output final reply → save session
```

- Concurrent-safe tools (Read, Grep, Glob) execute in parallel
- Non-concurrent tools (Write, Edit, Bash) execute sequentially
- Tool output is auto-truncated to prevent context window overflow
- Tool output can be compacted (see [Output Compaction](advanced.md#output-compaction))

## Tool Descriptions

Each built-in tool includes a detailed description and usage guidance that is injected into the system prompt. These descriptions help the LLM select the right tool and use it effectively — for example, preferring Grep over Bash for content search, or using Edit instead of Write for modifications.

## Script tool (W4)

The `Script` tool composes N built-in tool calls into one. It is
gated by `capabilities.rpc_tool_script` (W0 slot at events.rs:139);
the engine only registers it when `builtin_tools.script.enabled = true`
in wcore-config (default off).

### DSL

```jsonc
{
  "name": "Script",
  "input": {
    "steps": [
      { "id": "s1", "tool": "Grep", "input": { "pattern": "fn run(" } },
      { "id": "s2", "tool": "Read", "input": { "file_path": "${s1.matches.0.file}" } },
      { "id": "s3", "tool": "Edit",
        "input": { "file_path": "${s2.path}", "old_string": "...", "new_string": "..." },
        "approval_required": true }
    ],
    "max_output_lines": 200
  }
}
```

### Safety rails

- **Allow-list**: Read, Write, Edit, Grep, Glob, Bash, RepoMap. No
  SpawnTool, no recursive Script, no MCP tools, no plugin tools (W4
  scope).
- **Refs are json-path only**: `${stepId.field.subfield}`. No arithmetic,
  no shell, no expression language. Path syntax is `name(.name)*` where
  name is `[A-Za-z0-9_]+`.
- **approval_required: true** returns `is_error: true` with a clear
  message pre-W7 — the destructive step does NOT execute. W7 wires
  the formal `Suspend` event + resume-token round-trip.
- **max_output_lines** truncates the aggregated transcript; default 200.
- **Step failure short-circuits** — no half-applied state.

## RepoMap tool (W4, W3→W4 hand-off)

The `RepoMap` tool wraps `wcore_repomap::RepoMap::build` (shipped
standalone in W3) and `render::render_compact` behind the `Tool`
trait. Default-on per `[builtin_tools.repomap] enabled = true`; opt
out via `wcore.toml`. The tool is read-only by construction — it
walks the directory tree, never writes.

### Schema

```jsonc
{
  "name": "RepoMap",
  "input": {
    "query": "LlmProvider",          // optional substring filter
    "file_limit": 100,               // optional cap on rendered files
    "symbol_limit": 50               // optional cap on symbols per file
  }
}
```

### Behaviour

- `RepoMap::build` is offloaded via `tokio::task::spawn_blocking` so a
  5K-file index doesn't stall the runtime.
- `query` substring-filters `render_compact` output line-by-line
  (case-insensitive). Empty/missing query returns the full compact
  view.
- Output is truncated when it exceeds `file_limit × (symbol_limit + 1)`
  lines as a coarse upper bound; raise the limits for more detail.
- Read-only ⇒ `is_concurrency_safe(...)` returns `true` ⇒ Script may
  invoke `RepoMap` (in the allow-list above) without serialisation
  surprises.

## Browser tool family (W8c.1)

`Browser::*` tools are registered by the `wayland-browser` plugin
(via `wcore-browser`). Every op shares the ARIA-tree surface so
prompt budgets stay bounded.

Available ops (variants of `BrowserOp`):

| Op | Description |
|---|---|
| `Browser::navigate { url }` | Drive the active tab to the given URL. Gated by `BrowserPolicy`. |
| `Browser::snapshot` | Capture the current ARIA tree (default surface for LLM reasoning). |
| `Browser::click { selector }` | Click an element addressed by the ARIA-tree selector. |
| `Browser::type { selector, text }` | Type text into a focused field. |
| `Browser::new_tab { url? }` | Open a fresh tab (optionally pre-navigated). |
| `Browser::download { url }` | Download the resource at `url` to the workspace. |

Available capability flag: `capabilities.browser_suite` (W8c.1). The
engine emits `browser_event` and `browser_policy_denied` while ops
run; see `docs/json-stream-protocol.md` §§1.N+6 and 1.N+7.

## Computer use (W8c.2)

`Cua::*` tools are registered by the `wayland-cua` plugin (via
`wcore-cua`). Every op honours the background-mode invariant: no
foreground-app focus stealing.

Available ops (variants of `CuaOp`):

| Op | Description |
|---|---|
| `Cua::left_click { x, y }` / `right_click` / `middle_click` / `double_click` | Mouse button at screen coords. |
| `Cua::move_to { x, y }` | Move the cursor without clicking. |
| `Cua::drag { from, to }` | Press, move, release between two points. |
| `Cua::type { text }` | Type Unicode text into the focused app. |
| `Cua::key { combo }` | Send a key combo (e.g. `cmd+shift+4`). Blocks against `forbidden_key_combos`. |
| `Cua::screenshot` | Capture the screen; optionally redacted per `CuaPolicy`. |
| `Cua::ax_tree` | Capture the accessibility tree for the foreground app. |
| `Cua::wait { ms }` | Sleep without holding the runtime busy. |
| `Cua::frontmost_app` | Identifier of the current foreground app. |

Available capability flag: `capabilities.computer_use` (W8c.2). The
engine emits `cua_event` and `cua_policy_denied` while ops run; see
`docs/json-stream-protocol.md` §§1.N+8 and 1.N+9.

## IJFW tools (W8c.3)

`ijfw::*` tools are registered by the `wayland-ijfw` anchor plugin.
The tool bodies delegate to the registered IJFW MCP server
(`ijfw-memory`); both names below are addressable by the LLM through
the standard `tool_request` flow.

| Tool | Description |
|---|---|
| `ijfw::ijfw_run` | Run a query through the configured IJFW mode pipeline (smart / fast / deep / manual / brutal). |
| `ijfw::ijfw_update_apply` | Apply an IJFW update diff returned by `ijfw_update_check`. |

## Rollback (W8b F5)

The `Rollback` tool tier produces shadow snapshots of every file an
agent edits during a session (see `FileHistory` in `wcore-tools`).
Operators / hosts can request a `tool_result.metadata.rollback_token`
to checkpoint a state, then re-issue the token later via
`Rollback::restore { token }` to revert. Tokens are scoped to the
session and do NOT persist across restarts.

## Token-cost accounting (W12 B.4-tokens)

`tool_token_bench` (in `crates/wcore-agent/src/bin/`) is the
measurement harness for per-tool token-cost accounting. It dispatches
representative `ToolUse` calls through the production
`execute_tool_calls` path, captures the resulting `ToolResult.content`
strings, and emits a markdown table of
`(chars, heuristic_tokens, scripted_input_tokens, delta)` per tool.

Regenerate the scripted baseline:

```bash
vx cargo run --release -p wcore-agent \
    --bin tool_token_bench \
    --features test-utils
```

Output lands at `docs/tool-token-empirical-<UTC-date>.md`. Live-API
verification (real provider tokenization across Anthropic / OpenAI /
Bedrock / Vertex) is documented in §2 of the same doc and still
requires real credentials to fill in — that path is gated behind the
`live-api` Cargo feature on `wcore-agent` and currently exits with a
runbook pointer.

## Web search backends

The `web` tool (search / extract / crawl) dispatches through a pluggable
`WebBackend`. The active backend is chosen at startup by
`build_web_search_backend()`. **Every selected backend falls back to
DuckDuckGo on failure** (transport error, non-2xx, or no valid results),
so search never hard-fails — except when explicitly disabled.

**Selection order (first match wins):**

| Priority | Trigger | Backend |
|----------|---------|---------|
| override | `WAYLAND_WEB_BACKEND=off` | disabled (no fallback) |
| override | `WAYLAND_WEB_BACKEND=duckduckgo` | DuckDuckGo only |
| override | `WAYLAND_WEB_BACKEND=parallel` | Parallel free → DDG |
| 1 | `FIRECRAWL_API_KEY` (+ optional `FIRECRAWL_API_URL`) | Firecrawl → DDG |
| 2 | `PARALLEL_API_KEY` | Parallel REST → DDG |
| 3 | `TAVILY_API_KEY` | Tavily → DDG |
| 4 | `EXA_API_KEY` | Exa → DDG |
| 5 | `SEARXNG_URL` | SearXNG → DDG |
| 6 | `BRAVE_SEARCH_API_KEY` | Brave → DDG |
| default | *(no keys)* | **Parallel free → DDG** |

`WAYLAND_WEB_BACKEND` is an explicit override that wins over key presence;
`auto` (or unset / unrecognized) runs the ladder. The order matches the Hermes
agent's preference (firecrawl → parallel → tavily → exa → searxng → brave → ddg).

**Default (no config):** the engine uses Parallel.ai's free, anonymous Search
MCP (`https://search.parallel.ai/mcp`) — ranked URLs with query-relevant
excerpts, no API key. **Privacy:** your search queries are sent to parallel.ai.
A one-time log notes this on first use; set `WAYLAND_WEB_BACKEND=duckduckgo` to
keep queries on DuckDuckGo, or `=off` to disable web search entirely.

**SearXNG** is gated by `SEARXNG_URL` (your own or a public instance — the
engine ships the connector, not the instance). The instance must be **publicly
resolvable**: requests go through the SSRF-safe client, so a `SEARXNG_URL`
pointing at `localhost` / a private IP is rejected. (A scoped opt-in for
private SearXNG instances is a planned follow-up.)

API keys are redacted from logs / model context by `wcore-safety` PII scrubbing.
