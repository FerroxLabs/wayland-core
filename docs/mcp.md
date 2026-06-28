# MCP (Model Context Protocol) Integration

## Overview

MCP allows the agent to connect to external tool servers, extending beyond the 7 built-in tools to the entire MCP server ecosystem.

## Configuring MCP Servers

Declare MCP servers in the config file:

```toml
# Stdio transport: launch a local subprocess
[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/project"]

[mcp.servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_xxx" }

# SSE transport: connect to a remote SSE server
[mcp.servers.database]
transport = "sse"
url = "http://localhost:3001/sse"

# Streamable HTTP transport: HTTP POST communication
[mcp.servers.remote-tools]
transport = "streamable-http"
url = "https://tools.example.com/mcp"
headers = { Authorization = "Bearer xxx" }
```

## Zero-config discovery (Forge local servers)

Forge-suite desktop apps (e.g. Agent Vault) advertise a loopback MCP server by
writing a shared file at `<OS config dir>/forge/mcp-servers.json` — the *real*
config dir (`dirs::config_dir()`), NOT `WAYLAND_HOME`, since it is a
cross-application convention written by *other* apps about the actual machine.
Wayland Core auto-detects those entries and surfaces them as **DISCOVERED** rows
in `/doctor`. Nothing connects automatically: a discovery entry is a **hint, not
liveness** (a producer crash leaves a stale entry behind), so you connect a row
explicitly with `/mcp connect [name]` (or by selecting the row and pressing
Enter).

On connect, the engine runs the two-step bootstrap:

1. **Liveness probe** — `GET <metadata_url>`. The connect is only offered when a
   `200` comes back *and* the server's reported `name` matches the discovery
   entry. A mismatch (or any non-200) is treated as a stale/impostor entry and
   rejected.
2. **Grant** — `POST <grant_url>`. This pops an **Approve** prompt in the
   producer app; the call only returns successfully after the user clicks
   Approve. The response maps to a structured outcome:

   | Status | Outcome |
   |--------|---------|
   | `200` | Granted — a scoped bearer token is returned and stored, and the server connects live |
   | `403` | Denied (user declined, no UI, or timed out) |
   | `400` | Bad scopes — surfaced with the server's message |
   | `429` | Rate-limited — back off |

The token is stored out of `config.toml` via a credential reference (see
[Token storage](#token-storage-cred-references) below).

### Token storage (`${cred:}` references)

The scoped bearer token is never written into `config.toml`. Instead the
persisted `Authorization` header carries a `${cred:KEY}` reference, with the key
convention `mcp:<server>:token`. The literal `${cred:...}` stays on disk; the
real secret is looked up from the credentials store and substituted in **at the
connect boundary, on a clone** of the server map — so an accidental re-serialize
of the in-memory config can't leak the token back to disk. Resolution **fails
closed**: a missing key aborts the whole header value rather than emitting a
blank or half-resolved bearer.

The on-disk shape for a discovered Forge server:

```toml
[mcp.servers.agent-vault]
transport = "streamable-http"
url = "http://127.0.0.1:3456/mcp"
allow_local = true
[mcp.servers.agent-vault.headers]
Authorization = "Bearer ${cred:mcp:agent-vault:token}"
```

## Transport Types

| Transport | Description | Use Case |
|-----------|-------------|----------|
| `stdio` | Launch local subprocess, communicate via stdin/stdout | Local MCP servers (npx, uvx) |
| `sse` | GET for SSE event stream, POST for requests | Remote MCP servers |
| `streamable-http` | HTTP POST, supports SSE streaming responses | Remote MCP servers |

> **Windows stdio resolution.** On Windows, a `stdio` server launched by bare
> name (`npx`, `uvx`, `node`) resolves through `cmd /C` so that `.cmd`/`.bat`
> PATHEXT shims (`npx.cmd`, `node.cmd`) — which raw `CreateProcess` refuses to
> find on `PATH` — are resolved correctly. This is automatic; no host action or
> config knob is needed.

## Deferred Loading

MCP tools are registered as "deferred" **by default** — their full schema is not loaded into the system prompt at startup, reducing initial token usage. The LLM discovers deferred tools via the `ToolSearch` tool when needed. Set `deferred = false` on a server to send its full tool schemas eagerly instead.

```toml
[mcp.servers.small-toolset]
transport = "stdio"
command = "npx"
args = ["-y", "my-mcp-server"]
deferred = false    # Send full tool schemas at startup (opt out of deferral)
```

| `deferred` | Behavior |
|------------|----------|
| `true` (default when omitted) | Tools registered but schemas loaded on-demand via ToolSearch |
| `false` | Tool schemas included in system prompt at startup |

Leave `deferred` at its default (`true`) for MCP servers with many tools to keep the initial system prompt small; set `deferred = false` only for a small, always-needed toolset.

## Smart Tool Curation

A single large MCP server (e.g. Google Workspace) can advertise dozens or
hundreds of tools. Sending all of them on every turn wastes context tokens and,
on some providers, overflows the provider's tool-array limit (an OpenAI request
with more than 128 tools fails with an API 400). Wayland Core handles this with
two independent passes applied to the outbound tool list each turn, in this
order:

1. **Relevance curation** (`[mcp].curation`) — a token/relevance optimization
   that trims the MCP tools to the most relevant `k` for the turn.
2. **Provider hard cap** (`max_tools` compat field) — a correctness guarantee
   that caps the *total* tool array at the provider's limit.

Both passes only ever touch **MCP** tools. The built-in tools (Read, Write,
Edit, Bash, Grep, Glob, Spawn, ToolSearch, plan tools, skills) are always kept.

### How tools are classified (real provenance)

An MCP tool is identified by its **real provenance** — `ToolDef.server` is
`Some(server_name)` for any tool sourced from an MCP server, and `None` for a
built-in/skill/spawn/plan tool. Classification is **not** done on the `mcp__`
name prefix: a non-colliding MCP tool keeps its bare, un-prefixed original name
(see [Tool Naming](#tool-naming)), so the prefix alone cannot distinguish it
from a built-in. Using the prefix would misclassify a uniquely-named MCP tool as
a built-in and never curate or cap it.

### Relevance ranking (BM25 + recency)

Within the MCP partition, tools are ranked by:

```
score = BM25(user_message, tool_document) + recent_usage[tool] * 0.5
```

- **BM25 relevance** — the query is the most recent user message in the turn.
  Each tool's "document" is its `description` + the real server name + the
  tool-name tail (the last `__`-segment, or the bare name). The tokenizer
  lowercases, splits on non-alphanumerics, and drops tokens of length ≤ 3, so
  `mcp__gcal__list_calendar_events` tokenizes to `["gcal", "list", "calendar",
  "events"]`. BM25 parameters are `k1 = 1.5`, `b = 0.75`, with Robertson IDF
  guarded by the BM25+ `+1.0` term (keeps IDF non-negative for a term present in
  every tool). This mirrors the desktop app's `bm25.ts` for cross-surface
  parity.
- **Recency boost** — an additive `0.5` per recent use, read from the long-term
  audit log. When no audit log is available the curator gracefully degrades to
  BM25-only ranking.

There is deliberately **no** name-keyed "rescue" bonus. Because only MCP tools
are ranked here, a bare-name floor (e.g. matching the name `Read`) could only
ever reward a hostile MCP server that names a tool like a built-in to monopolize
the curation budget — a budget-hijack vector closed in 0.12.11. Built-ins are
kept by the caller, outside the curator.

### Configuring relevance curation — `[mcp].curation`

```toml
# Default: keep the 15 most relevant MCP tools per turn.
[mcp.curation]
kind = "top_k"
k = 15

# Or disable curation entirely (expose every connected MCP tool).
[mcp.curation]
kind = "off"
```

| `[mcp].curation` `kind` | Behavior |
|-------------------------|----------|
| `top_k` (default, `k = 15`) | Trim the per-turn MCP tool list to the `k` highest-ranked tools |
| `off` | Expose every connected MCP tool (no relevance trim) |

When the connected MCP tool count is already ≤ `k`, curation is a no-op.

### Provider hard cap — `max_tools`

After relevance curation, the engine enforces the provider's hard tool-array
limit. This is a `ProviderCompat` field, set per-provider:

| Provider | `max_tools` default |
|----------|---------------------|
| OpenAI / OpenAI-wire (Azure, ChatGPT, flux-router, routers, …) | `128` |
| Anthropic (and providers that don't set it) | none (uncapped) |

The cap keeps **all** non-MCP tools, then fills the remaining budget with the
most BM25-relevant MCP tools and truncates the rest. If the built-ins alone
meet or exceed the limit, the entire MCP block is dropped for that request. You
can override the cap per provider in `wcore.toml`:

```toml
[providers.openai.compat]
max_tools = 64
```

### Dropped tools stay reachable

MCP tools trimmed by either pass are not lost — they remain **discoverable via
the `ToolSearch` meta-tool**, whose registry is a full bootstrap snapshot. The
model can search for and surface a curated-out tool when a turn actually needs
it.

### Cache stability

Both passes emit the kept MCP set in an **append-only union order** keyed on the
MCP tool inventory (the hard cap additionally keys on its budget). A tool, once
admitted, holds its slot; newly-surfaced tools are appended at the end. This
keeps the serialized tool-zone prefix byte-stable across same-context turns so
the provider prompt cache is read, not rewritten, every turn. The union resets
only when the inventory itself changes (server connect/disconnect or plugin
reload).

## Local (loopback) MCP servers — `allow_local`

For safety, the HTTP transports (`sse`, `streamable-http`) refuse to connect to
URLs that resolve to private/internal addresses — an SSRF guard that stops a
compromised or model-driven URL from reaching `169.254.169.254`, your LAN, etc.
By default this also blocks **loopback** (`127.0.0.1`, `::1`, `localhost`).

An MCP server you configure by hand is *trusted configuration*, not a
model-supplied URL, so to connect to a local MCP server (e.g. a desktop app
exposing tools on `127.0.0.1`) set `allow_local = true`. This relaxes the
**loopback block only** — every other private/LAN/link-local/CGNAT/cloud-metadata
range stays blocked even when enabled.

```toml
# Example: connect to Agent Vault (desktop) running a local MCP server.
[mcp.servers.agent-vault]
transport = "streamable-http"
url = "http://127.0.0.1:3456/mcp"
allow_local = true
headers = { Authorization = "Bearer <AGENT_VAULT_MCP_TOKEN>" }
```

| `allow_local` | Behavior |
|---------------|----------|
| `false` (default) | Loopback and all private/internal targets are rejected at connect time |
| `true` | Loopback (`127.0.0.0/8`, `::1`, `localhost`) is permitted; all other private/internal/metadata ranges remain blocked |

To keep the bearer token out of `config.toml`, a header value may use a
`${cred:KEY}` reference (e.g. `Authorization = "Bearer ${cred:mcp:agent-vault:token}"`)
— the literal reference is stored on disk and the secret is resolved from the
credentials store at connect time. See [Token storage](#token-storage-cred-references).

## Tool Naming

- MCP tool names are used directly when there's no conflict
- On conflict with built-in or other MCP tools, names are auto-prefixed: `mcp__{server}__{tool}`

## Startup Flow

1. Connect to all configured MCP servers
2. Perform MCP protocol handshake (`initialize`) for each server
3. Discover available tools (`tools/list`)
4. Register tools in the tool registry — the agent uses them like built-in tools
5. Gracefully close all connections on exit

## Plugin Lifecycle Hooks → Context

A plugin can register **lifecycle hooks** that contribute text into the model's
context at well-defined points. Two phases dispatch a contribution today:

- **SessionStart** — fires once on a *cold* session (no prior conversation). The
  contribution is folded in as the first message (e.g. a memory prelude). On a
  resumed session it is skipped (the restored history already carries context).
- **PrePrompt** — fires once per user turn, immediately before the request is
  streamed (e.g. per-turn recall).

The dispatch resolves a hook to an MCP tool of the **same name** on the plugin's
MCP server, calls it, and wraps the result as an *untrusted* block:

```
<plugin-context source="{plugin}:{hook}" trust="untrusted"> … </plugin-context>
```

This block is always a **user-role** message on the volatile tail — it never
enters the system prompt and never shifts the cached system+tools prefix. Tool
output is treated as data, not instructions, and host trust-tag delimiters in
the body are defanged so a backend can't forge host framing. Other phases
(`PostToolUse`, `SessionEnd`, `PreCompact`) are currently log-only.

A plugin binds to a server only when the match is **unambiguous** (exactly one
connected server advertises a tool matching the hook name). If two servers
advertise the same name the binding is refused and the hook stays log-only.

**Kill-switch:** `hooks.dispatch_enabled` (default `true`) disables all hook→
context dispatch when set to `false`, leaving plugins and MCP otherwise intact.

## Plugin MCP Server Home (`~/.wayland`)

Plugin installers write under `~/.wayland` (the *profile home*), and the host
exposes that same root to launched plugin MCP servers so a server can find its
installed assets. The resolution order is:

1. `$WAYLAND_PROFILE_HOME` / `$WAYLAND_HOME` when set (sandbox / hermetic
   override; ignored if it contains control characters)
2. `~/.wayland` (the cross-platform default)

This is framework-neutral: any plugin that ships an MCP server uses the same
handshake.
