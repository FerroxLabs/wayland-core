"""Owned loopback-test MCP server; holds one tool with a real OS child."""

import json
import os
from pathlib import Path
import subprocess
import sys
import time

root = Path(sys.argv[1])
(root / "mcp-parent.pid").write_text(str(os.getpid()))

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "lifecycle-child", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "w02_hold_child",
            "description": "Hold an owned test child until session cleanup.",
            "inputSchema": {"type": "object", "properties": {}},
        }]}
    elif method == "tools/call":
        child_code = (
            "import os, pathlib, sys, time\n"
            "root = pathlib.Path(sys.argv[1])\n"
            "(root / 'mcp-child.pid').write_text(str(os.getpid()))\n"
            "while not (root / 'release-child').exists(): time.sleep(0.02)\n"
            "(root / 'child-effect.txt').write_text('child continued')\n"
        )
        child = subprocess.Popen([sys.executable, "-c", child_code, str(root)])
        while not (root / "mcp-child.pid").exists():
            time.sleep(0.01)
        marker = root / "running.json.tmp"
        marker.write_text(json.dumps({"parent": os.getpid(), "child": child.pid}))
        marker.replace(root / "running.json")
        # Deliberately no response: cancellation must close the real transport
        # and its process group, not mistake a fake success for completion.
        continue
    elif method == "resources/list":
        result = {"resources": []}
    elif method == "prompts/list":
        result = {"prompts": []}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {
            "code": -32601, "message": "method not found",
        }}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
