#!/usr/bin/env python3
"""Prove the A-11 job is completable, and that the three shortcuts are not.

Speaks MCP to the server over stdio, exactly as a client would, and runs four
scenarios:

  competent          the job done properly                 -> PASS
  substring_tool     resolved `reserve` by substring        -> FAIL
  no_catalogue_refresh  never re-listed tools after commit  -> FAIL
  wrong_actor        acted as an unauthorised actor         -> FAIL

The last three are the three defects that made MCP tools uncallable in this
product. If any of them still reaches PASS, this fixture would not have caught
them and it needs rebuilding.
"""

import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SERVER = os.path.join(ROOT, "fixtures", "a11_mcp", "warehouse_mcp.py")
VERIFIER = os.path.join(HERE, "a11_verify.py")


class Client:
    def __init__(self, db):
        env = dict(os.environ)
        env["WAREHOUSE_TOKEN"] = "test-token-not-a-real-credential"
        self.proc = subprocess.Popen(
            [sys.executable, SERVER, "--db", db],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            env=env, text=True, bufsize=1,
        )
        self.next_id = 0
        self.notifications = []
        self.tools = []
        self.call("initialize", {})
        self.refresh_tools()

    def call(self, method, params):
        self.next_id += 1
        request = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("server closed the connection")
            message = json.loads(line)
            if "id" not in message:
                self.notifications.append(message.get("method"))
                continue
            return message

    def refresh_tools(self):
        result = self.call("tools/list", {})
        self.tools = [t["name"] for t in result["result"]["tools"]]
        return self.tools

    def tool(self, name, **arguments):
        response = self.call("tools/call", {"name": name, "arguments": arguments})
        if "error" in response:
            return {"transport_error": response["error"]["message"]}
        result = response["result"]
        text = result["content"][0]["text"]
        return {"isError": result.get("isError", False), "text": text}

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            self.proc.kill()


def competent(client, export_path):
    reason = "order SO-88120"
    ids = []
    for sku, quantity in (("SKU-4410", 6), ("SKU-1180", 25)):
        out = client.tool("inventory_reserve", sku=sku, quantity=quantity,
                          actor="dispatch-bot", reason=reason)
        ids.append(json.loads(out["text"])["reservation_id"])
    # The line that cannot be filled: attempt it, accept the refusal, leave it.
    short = client.tool("inventory_reserve", sku="SKU-2245", quantity=15,
                        actor="dispatch-bot", reason=reason)
    assert short["isError"], "SKU-2245 should not have been reservable"
    for reservation in ids:
        client.tool("inventory_commit", reservation_id=reservation,
                    actor="dispatch-bot", reason=reason)
    # The export tool did not exist a moment ago.
    client.refresh_tools()
    client.tool("inventory_audit_export", path=export_path, actor="dispatch-bot")


def substring_tool(client, export_path):
    """A client that resolved the tool name by substring gets the retired stub."""
    reason = "order SO-88120"
    for sku, quantity in (("SKU-4410", 6), ("SKU-1180", 25)):
        client.tool("reserve", sku=sku, quantity=quantity,
                    actor="dispatch-bot", reason=reason)
    client.refresh_tools()
    client.tool("inventory_audit_export", path=export_path, actor="dispatch-bot")


def no_catalogue_refresh(client, export_path):
    """Everything right, except the catalogue is never re-read."""
    reason = "order SO-88120"
    ids = []
    for sku, quantity in (("SKU-4410", 6), ("SKU-1180", 25)):
        out = client.tool("inventory_reserve", sku=sku, quantity=quantity,
                          actor="dispatch-bot", reason=reason)
        ids.append(json.loads(out["text"])["reservation_id"])
    client.tool("inventory_reserve", sku="SKU-2245", quantity=15,
                actor="dispatch-bot", reason=reason)
    for reservation in ids:
        client.tool("inventory_commit", reservation_id=reservation,
                    actor="dispatch-bot", reason=reason)
    # No refresh_tools() here, so inventory_audit_export is not in the catalogue
    # this client believes in, and the export never happens.


def wrong_actor(client, export_path):
    reason = "order SO-88120"
    for sku, quantity in (("SKU-4410", 6), ("SKU-1180", 25)):
        client.tool("inventory_reserve", sku=sku, quantity=quantity,
                    actor="curious-intern", reason=reason)
    client.refresh_tools()
    client.tool("inventory_audit_export", path=export_path, actor="curious-intern")


SCENARIOS = [
    ("competent", competent, "PASS"),
    ("substring_tool", substring_tool, "FAIL"),
    ("no_catalogue_refresh", no_catalogue_refresh, "FAIL"),
    ("wrong_actor", wrong_actor, "FAIL"),
]


def main():
    results = {}
    ok = True
    for name, scenario, expected in SCENARIOS:
        work = tempfile.mkdtemp(prefix="a11-%s-" % name)
        db = os.path.join(work, "warehouse.db")
        export = os.path.join(work, "audit-export.json")
        client = Client(db)
        try:
            scenario(client, export)
        finally:
            client.close()
        proc = subprocess.run(
            [sys.executable, VERIFIER, "--db", db, "--export", export],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        report = json.loads(proc.stdout.decode("utf-8"))
        got = report["verdict"]
        agrees = got == expected
        ok = ok and agrees
        results[name] = {
            "expected": expected, "got": got, "agrees": agrees,
            "reasons": report["reasons"][:4],
            "saw_list_changed_notification": "notifications/tools/list_changed"
            in getattr(client, "notifications", []),
        }

    print(json.dumps(results, indent=2))
    print("\nA-11 fixture self-test: %s" % ("OK" if ok else "BROKEN"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
