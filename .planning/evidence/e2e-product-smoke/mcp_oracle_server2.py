#!/usr/bin/env python3
"""Minimal MCP stdio server for the E2E product smoke journey.

WHY A PURPOSE-BUILT SERVER RATHER THAN `wayland-core mcp-serve`. Step 6 asks
whether the product can CONNECT to an MCP server and CALL a tool through it.
Pointing the product at its own `mcp-serve` would make a pass consistent with
"the two halves of one binary agree", which is a weaker claim. This server is
independent of the product entirely, and it advertises exactly one tool whose
return value is a NONCE the model cannot possibly know any other way.

That nonce is the whole design. A model can hallucinate "I called the tool".
It cannot hallucinate a random 128-bit token that exists only in this
process's memory and in the file this process writes. So step 6 is graded on
the nonce appearing in the product's stdout AND on this server's own call log
recording the invocation -- two independent positives, neither of which the
model can fake.

Protocol: JSON-RPC 2.0, newline-delimited, over stdin/stdout.
Version pinned to 2025-03-26 to match `wcore-mcp`'s client.

Every request and response is appended to $E2E_MCP_LOG so the caller can
verify from the SERVER's side that a call really arrived.
"""

import json
import os
import sys

NONCE = os.environ.get("E2E_MCP_NONCE", "no-nonce-set")
LOG = os.environ.get("E2E_MCP_LOG", "/tmp/e2e-mcp-oracle.log")

TOOLS = [
    {
        "name": "e2e_oracle",
        "description": (
            "Returns the E2E smoke-test oracle token. Call this with no "
            "arguments to obtain the token."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    }
]


def log(kind, payload):
    try:
        with open(LOG, "a") as fh:
            fh.write(json.dumps({"kind": kind, "payload": payload}) + "\n")
            fh.flush()
    except Exception:
        pass


def send(obj):
    log("out", obj)
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    log("start", {"pid": os.getpid()})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as exc:  # malformed frame -- record and continue
            log("badframe", {"line": line[:200], "err": str(exc)})
            continue
        log("in", req)

        method = req.get("method")
        rid = req.get("id")

        # Notifications carry no id and take no response.
        if rid is None:
            continue

        if method == "initialize":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "e2e-oracle", "version": "1.0.0"},
                    },
                }
            )
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            params = req.get("params") or {}
            name = params.get("name")
            if name == "e2e_oracle":
                log("ORACLE_CALLED", {"id": rid})
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": rid,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": f"The oracle token is {NONCE} and nothing else.",
                                }
                            ],
                            "isError": False,
                        },
                    }
                )
            else:
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": rid,
                        "error": {"code": -32602, "message": f"unknown tool {name}"},
                    }
                )
        else:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {"code": -32601, "message": f"unknown method {method}"},
                }
            )
    log("eof", {})


if __name__ == "__main__":
    main()
