#!/usr/bin/env python3
"""Minimal mock stdio MCP server for the wcore-eval-scenarios D6 round-trip.

Speaks MCP JSON-RPC 2.0 over stdin/stdout (one JSON object per line, the
"line-delimited" framing wayland-core's stdio transport expects). It is
deliberately tiny and dependency-free (stdlib only) so it runs anywhere
`python3` resolves on PATH.

Protocol surface implemented (enough for the wayland-core handshake):
  - `initialize`            → advertise protocolVersion + serverInfo + capabilities
  - `notifications/initialized` (notification, no id) → ignored
  - `tools/list`            → advertise ONE tool: `mcp_echo({text}) -> text`
  - `tools/call`            → for `mcp_echo`, return the supplied `text` verbatim
  - anything else with an id → JSON-RPC error -32601 (method not found)

`mcp_echo` returns its `text` argument unchanged inside a single MCP
`content` text block, so a scenario can plant a sentinel string in the
prompt and assert it round-tripped through the real engine -> real MCP
client -> this server -> back.

Robustness notes:
  - Blank lines are skipped (some clients flush newlines).
  - Notifications (messages with no `id`) never get a response, per JSON-RPC.
  - Unparseable input lines are ignored rather than crashing the handshake.
  - Every response is flushed immediately so the client's read loop unblocks.
"""

import json
import sys

# The tool this server advertises. Single tool keeps the round-trip
# unambiguous: whatever name fires in the trace is THIS one.
TOOL_NAME = "mcp_echo"

TOOL_DEF = {
    "name": TOOL_NAME,
    "description": "Echo back the provided text verbatim. Use this to repeat a string exactly.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "The exact text to echo back.",
            }
        },
        "required": ["text"],
    },
}


def _send(message):
    """Write one JSON object as a single line and flush."""
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _handle(req):
    method = req.get("method")
    req_id = req.get("id")

    # Notifications (no id) get no response per JSON-RPC 2.0.
    if req_id is None:
        return

    if method == "initialize":
        _result(
            req_id,
            {
                # Echo back a protocol version the client will accept; the
                # client requests one in params — mirror it when present.
                "protocolVersion": req.get("params", {}).get(
                    "protocolVersion", "2024-11-05"
                ),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-echo", "version": "0.1.0"},
            },
        )
    elif method == "tools/list":
        _result(req_id, {"tools": [TOOL_DEF]})
    elif method == "tools/call":
        params = req.get("params", {})
        name = params.get("name")
        if name != TOOL_NAME:
            _error(req_id, -32602, f"unknown tool: {name!r}")
            return
        text = params.get("arguments", {}).get("text", "")
        _result(
            req_id,
            {
                "content": [{"type": "text", "text": text}],
                "isError": False,
            },
        )
    else:
        _error(req_id, -32601, f"method not found: {method!r}")


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            # Ignore unparseable lines rather than killing the handshake.
            continue
        if isinstance(req, dict):
            _handle(req)


if __name__ == "__main__":
    main()
