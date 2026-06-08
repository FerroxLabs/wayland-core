# mcp-server-time

Tiny demo Model Context Protocol (MCP) server. Used by the `plugin-subprocess-mcp` Wayland example.

- Protocol: MCP JSON-RPC 2.0 over stdio (newline-delimited frames).
- Tool surface: one tool, `get_time(timezone: string) -> string`.
- Time semantics: returns seconds-since-UNIX-epoch as a UTC value. The `timezone` argument is echoed back as a display label. A production wrapper would resolve the timezone via `chrono-tz`; this demo deliberately stays minimal.

This crate is **not** a member of the main Wayland workspace. It is a downstream consumer — exactly the shape a third-party MCP server would have.

## Build

```bash
cd examples/plugin-subprocess-mcp/mcp-server-time
cargo build --release
```

The binary lands at `target/release/mcp-server-time`.

## Manual smoke

```bash
# Send an initialize + tools/list and watch the responses scroll by.
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_time","arguments":{"timezone":"UTC"}}}' \
| ./target/release/mcp-server-time
```

You should see four JSON-RPC response lines (initialize result, no reply for the notification, tools/list result, tools/call result with text content).
