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

## Transport Types

| Transport | Description | Use Case |
|-----------|-------------|----------|
| `stdio` | Launch local subprocess, communicate via stdin/stdout | Local MCP servers (npx, uvx) |
| `sse` | GET for SSE event stream, POST for requests | Remote MCP servers |
| `streamable-http` | HTTP POST, supports SSE streaming responses | Remote MCP servers |

## Deferred Loading

MCP tools can be registered as "deferred" — their full schema is not loaded into the system prompt at startup, reducing initial token usage. The LLM discovers deferred tools via the `ToolSearch` tool when needed.

```toml
[mcp.servers.large-toolset]
transport = "stdio"
command = "npx"
args = ["-y", "my-mcp-server"]
deferred = true    # Don't load tool schemas at startup
```

| `deferred` | Behavior |
|------------|----------|
| `false` (default for config servers) | Tool schemas included in system prompt at startup |
| `true` | Tools registered but schemas loaded on-demand via ToolSearch |

Use `deferred = true` for MCP servers with many tools to keep the initial system prompt small.

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
