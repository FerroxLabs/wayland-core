#!/usr/bin/env python3
"""A stateful MCP stdio server for A-11: warehouse inventory with authorisation.

Standard library only. State lives in a SQLite file the grader opens directly,
so the external effect is verified without asking the agent or the server what
happened.

This is not an echo server, and it is built so that the three defects that made
MCP tools uncallable in this product until recently would each stop the job
dead:

1. **Name matching.** The catalogue contains `reserve`, `inventory_reserve` and
   `inventory_reserve_bulk`. Only `inventory_reserve` does the job. `reserve`
   is a deprecated stub that refuses, and `inventory_reserve_bulk` needs a
   different argument shape. A client that resolves a tool by substring gets
   the wrong one and the audit log records it.

2. **Catalogue refresh.** `inventory_audit_export` does not exist when the
   session starts. It is registered the first time a reservation is committed,
   and the server then sends `notifications/tools/list_changed`. The job cannot
   be finished without calling a tool that appeared mid-session, so a client
   that registers tools once and never refreshes simply cannot complete.

3. **Callability.** `inventory_purge` is advertised and always refuses. Every
   mutating tool refuses outright for an actor outside the allow list, and
   refusals never touch inventory. Being listed is not being callable, and the
   grader checks the database, not the answer.

Run as: python3 warehouse_mcp.py --db <path>
"""

import argparse
import json
import os
import sqlite3
import sys
import threading
import time

PROTOCOL_VERSION = "2024-11-05"
ALLOWED_ACTORS = ("dispatch-bot", "warehouse-lead")
REQUIRED_TOKEN_ENV = "WAREHOUSE_TOKEN"

_lock = threading.Lock()
_state = {"export_tool_registered": False}


# --------------------------------------------------------------------------
# storage
# --------------------------------------------------------------------------

def connect(path):
    conn = sqlite3.connect(path, check_same_thread=False)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS stock (
            sku TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            on_hand INTEGER NOT NULL,
            reserved INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS reservations (
            id TEXT PRIMARY KEY,
            sku TEXT NOT NULL,
            quantity INTEGER NOT NULL,
            state TEXT NOT NULL,
            actor TEXT NOT NULL,
            created_at REAL NOT NULL,
            committed_at REAL
        );
        CREATE TABLE IF NOT EXISTS audit (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            at REAL NOT NULL,
            actor TEXT NOT NULL,
            tool TEXT NOT NULL,
            reason TEXT,
            outcome TEXT NOT NULL,
            detail TEXT
        );
        """
    )
    conn.commit()
    return conn


SEED = [
    ("SKU-1180", "Insulated shipper, 12L", 40),
    ("SKU-2245", "Pallet wrap, 500m", 12),
    ("SKU-3390", "Thermal label roll", 250),
    ("SKU-4410", "Gel pack, 400g", 8),
]


def seed(conn):
    for sku, description, on_hand in SEED:
        conn.execute(
            "INSERT OR IGNORE INTO stock (sku, description, on_hand, reserved) VALUES (?,?,?,0)",
            (sku, description, on_hand),
        )
    conn.commit()


def record(conn, actor, tool, reason, outcome, detail=None):
    conn.execute(
        "INSERT INTO audit (at, actor, tool, reason, outcome, detail) VALUES (?,?,?,?,?,?)",
        (time.time(), actor or "<none>", tool, reason, outcome,
         json.dumps(detail) if detail is not None else None),
    )
    conn.commit()


# --------------------------------------------------------------------------
# authorisation
# --------------------------------------------------------------------------

class Refused(Exception):
    pass


def authorise(conn, tool, args, mutating):
    actor = args.get("actor")
    reason = args.get("reason")
    token = os.environ.get(REQUIRED_TOKEN_ENV)

    if not token:
        record(conn, actor, tool, reason, "refused", "server has no credential")
        raise Refused("this server was started without %s and cannot act" % REQUIRED_TOKEN_ENV)
    if actor not in ALLOWED_ACTORS:
        record(conn, actor, tool, reason, "refused", "actor not on the allow list")
        raise Refused(
            "actor %r is not authorised. Authorised actors: %s"
            % (actor, ", ".join(ALLOWED_ACTORS))
        )
    if mutating and not reason:
        record(conn, actor, tool, reason, "refused", "no reason supplied")
        raise Refused("a mutating call must carry a reason")
    return actor, reason


# --------------------------------------------------------------------------
# tools
# --------------------------------------------------------------------------

def tool_list(conn, args):
    rows = conn.execute(
        "SELECT sku, description, on_hand, reserved FROM stock ORDER BY sku"
    ).fetchall()
    return {
        "items": [
            {"sku": r[0], "description": r[1], "on_hand": r[2],
             "reserved": r[3], "available": r[2] - r[3]}
            for r in rows
        ]
    }


def tool_reserve(conn, args):
    actor, reason = authorise(conn, "inventory_reserve", args, mutating=True)
    sku = args.get("sku")
    quantity = args.get("quantity")
    if not isinstance(quantity, int) or quantity <= 0:
        raise Refused("quantity must be a positive integer")
    row = conn.execute("SELECT on_hand, reserved FROM stock WHERE sku = ?", (sku,)).fetchone()
    if row is None:
        record(conn, actor, "inventory_reserve", reason, "refused", "unknown sku %s" % sku)
        raise Refused("unknown sku %r" % sku)
    available = row[0] - row[1]
    if quantity > available:
        record(conn, actor, "inventory_reserve", reason, "refused",
               {"sku": sku, "wanted": quantity, "available": available})
        raise Refused(
            "only %d of %s are available; %d were requested" % (available, sku, quantity)
        )
    reservation_id = "RES-%s-%04d" % (
        sku.split("-")[1],
        (conn.execute("SELECT COUNT(*) FROM reservations").fetchone()[0] + 1),
    )
    conn.execute(
        "INSERT INTO reservations (id, sku, quantity, state, actor, created_at) "
        "VALUES (?,?,?,'held',?,?)",
        (reservation_id, sku, quantity, actor, time.time()),
    )
    conn.execute("UPDATE stock SET reserved = reserved + ? WHERE sku = ?", (quantity, sku))
    conn.commit()
    record(conn, actor, "inventory_reserve", reason, "ok",
           {"reservation": reservation_id, "sku": sku, "quantity": quantity})
    return {"reservation_id": reservation_id, "sku": sku, "quantity": quantity, "state": "held"}


def tool_commit(conn, args):
    actor, reason = authorise(conn, "inventory_commit", args, mutating=True)
    reservation_id = args.get("reservation_id")
    row = conn.execute(
        "SELECT sku, quantity, state FROM reservations WHERE id = ?", (reservation_id,)
    ).fetchone()
    if row is None:
        record(conn, actor, "inventory_commit", reason, "refused",
               "unknown reservation %s" % reservation_id)
        raise Refused("unknown reservation %r" % reservation_id)
    sku, quantity, state = row
    if state == "committed":
        # Committing twice must not decrement twice. The refusal is recorded so
        # a double commit is visible to the grader even though it changed nothing.
        record(conn, actor, "inventory_commit", reason, "refused",
               {"reservation": reservation_id, "why": "already committed"})
        raise Refused("reservation %s was already committed" % reservation_id)
    if state != "held":
        raise Refused("reservation %s is %s and cannot be committed" % (reservation_id, state))

    conn.execute(
        "UPDATE stock SET on_hand = on_hand - ?, reserved = reserved - ? WHERE sku = ?",
        (quantity, quantity, sku),
    )
    conn.execute(
        "UPDATE reservations SET state = 'committed', committed_at = ? WHERE id = ?",
        (time.time(), reservation_id),
    )
    conn.commit()
    record(conn, actor, "inventory_commit", reason, "ok",
           {"reservation": reservation_id, "sku": sku, "quantity": quantity})

    with _lock:
        newly = not _state["export_tool_registered"]
        _state["export_tool_registered"] = True
    return {
        "reservation_id": reservation_id, "sku": sku, "quantity": quantity,
        "state": "committed",
        "_tools_changed": newly,
    }


def tool_release(conn, args):
    actor, reason = authorise(conn, "inventory_release", args, mutating=True)
    reservation_id = args.get("reservation_id")
    row = conn.execute(
        "SELECT sku, quantity, state FROM reservations WHERE id = ?", (reservation_id,)
    ).fetchone()
    if row is None:
        raise Refused("unknown reservation %r" % reservation_id)
    sku, quantity, state = row
    if state != "held":
        raise Refused("reservation %s is %s and cannot be released" % (reservation_id, state))
    conn.execute("UPDATE stock SET reserved = reserved - ? WHERE sku = ?", (quantity, sku))
    conn.execute("UPDATE reservations SET state = 'released' WHERE id = ?", (reservation_id,))
    conn.commit()
    record(conn, actor, "inventory_release", reason, "ok",
           {"reservation": reservation_id, "sku": sku, "quantity": quantity})
    return {"reservation_id": reservation_id, "state": "released"}


def tool_reserve_deprecated(conn, args):
    record(conn, args.get("actor"), "reserve", args.get("reason"), "refused",
           "deprecated stub was called instead of inventory_reserve")
    raise Refused(
        "`reserve` was retired in v2. Use `inventory_reserve`. This call changed nothing."
    )


def tool_reserve_bulk(conn, args):
    actor, reason = authorise(conn, "inventory_reserve_bulk", args, mutating=True)
    lines = args.get("lines")
    if not isinstance(lines, list) or not lines:
        record(conn, actor, "inventory_reserve_bulk", reason, "refused", "no lines supplied")
        raise Refused("inventory_reserve_bulk takes a `lines` array of {sku, quantity}")
    results = []
    for line in lines:
        results.append(tool_reserve(conn, dict(args, sku=line.get("sku"),
                                               quantity=line.get("quantity"))))
    return {"reservations": results}


def tool_purge(conn, args):
    record(conn, args.get("actor"), "inventory_purge", args.get("reason"), "refused",
           "purge is never permitted")
    raise Refused("inventory_purge is not permitted on this deployment.")


def tool_audit_export(conn, args):
    actor, _reason = authorise(conn, "inventory_audit_export", args, mutating=False)
    path = args.get("path")
    if not path:
        raise Refused("path is required")
    rows = conn.execute(
        "SELECT seq, at, actor, tool, reason, outcome, detail FROM audit ORDER BY seq"
    ).fetchall()
    payload = {
        "exported_by": actor,
        "entries": [
            {"seq": r[0], "actor": r[2], "tool": r[3], "reason": r[4],
             "outcome": r[5], "detail": json.loads(r[6]) if r[6] else None}
            for r in rows
        ],
    }
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)
    record(conn, actor, "inventory_audit_export", None, "ok",
           {"path": path, "entries": len(rows)})
    return {"path": path, "entries": len(rows)}


BASE_TOOLS = {
    "inventory_list": (tool_list, "List every SKU with what is on hand and what is reserved.", {
        "type": "object", "properties": {}, "additionalProperties": False}),
    "inventory_reserve": (tool_reserve, "Hold stock against a new reservation.", {
        "type": "object",
        "properties": {
            "sku": {"type": "string"},
            "quantity": {"type": "integer", "minimum": 1},
            "actor": {"type": "string", "description": "who is asking; must be authorised"},
            "reason": {"type": "string", "description": "why; required for any change"},
        },
        "required": ["sku", "quantity", "actor", "reason"]}),
    "inventory_reserve_bulk": (tool_reserve_bulk, "Hold stock for several SKUs at once.", {
        "type": "object",
        "properties": {
            "lines": {"type": "array", "items": {
                "type": "object",
                "properties": {"sku": {"type": "string"},
                               "quantity": {"type": "integer", "minimum": 1}},
                "required": ["sku", "quantity"]}},
            "actor": {"type": "string"},
            "reason": {"type": "string"},
        },
        "required": ["lines", "actor", "reason"]}),
    "reserve": (tool_reserve_deprecated,
                "DEPRECATED. Retired in v2 and always refuses. Use inventory_reserve.", {
        "type": "object", "properties": {"actor": {"type": "string"}}}),
    "inventory_commit": (tool_commit, "Turn a held reservation into a despatch, once.", {
        "type": "object",
        "properties": {
            "reservation_id": {"type": "string"},
            "actor": {"type": "string"},
            "reason": {"type": "string"},
        },
        "required": ["reservation_id", "actor", "reason"]}),
    "inventory_release": (tool_release, "Give a held reservation back to free stock.", {
        "type": "object",
        "properties": {
            "reservation_id": {"type": "string"},
            "actor": {"type": "string"},
            "reason": {"type": "string"},
        },
        "required": ["reservation_id", "actor", "reason"]}),
    "inventory_purge": (tool_purge, "Delete all records. Never permitted here.", {
        "type": "object", "properties": {"actor": {"type": "string"}}}),
}

LATE_TOOLS = {
    "inventory_audit_export": (
        tool_audit_export,
        "Write the full audit trail to a file. Only available once something has been "
        "committed.",
        {"type": "object",
         "properties": {"path": {"type": "string"}, "actor": {"type": "string"}},
         "required": ["path", "actor"]}),
}

# Filler so the catalogue is big enough that bulk registration is real work
# rather than a special case of one.
for _i in range(34):
    def _make(index):
        def handler(conn, args, index=index):
            return {"zone": "Z%02d" % index, "temperature_c": 4 + (index % 7)}
        return handler
    BASE_TOOLS["warehouse_zone_%02d_status" % _i] = (
        _make(_i), "Read the environmental status of storage zone Z%02d." % _i,
        {"type": "object", "properties": {}, "additionalProperties": False})


def visible_tools():
    tools = dict(BASE_TOOLS)
    if _state["export_tool_registered"]:
        tools.update(LATE_TOOLS)
    return tools


def descriptor(name, entry):
    _handler, description, schema = entry
    return {"name": name, "description": description, "inputSchema": schema}


# --------------------------------------------------------------------------
# JSON-RPC over stdio
# --------------------------------------------------------------------------

def write(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    args = ap.parse_args()
    conn = connect(args.db)
    seed(conn)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except ValueError:
            continue

        method = request.get("method")
        request_id = request.get("id")
        params = request.get("params") or {}

        if method == "initialize":
            write({"jsonrpc": "2.0", "id": request_id, "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": True}},
                "serverInfo": {"name": "warehouse", "version": "2.0.0"},
            }})
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            write({"jsonrpc": "2.0", "id": request_id, "result": {
                "tools": [descriptor(n, e) for n, e in sorted(visible_tools().items())]
            }})
        elif method == "tools/call":
            name = params.get("name")
            call_args = params.get("arguments") or {}
            tools = visible_tools()
            if name not in tools:
                write({"jsonrpc": "2.0", "id": request_id, "error": {
                    "code": -32601, "message": "no tool named %r" % name}})
                continue
            handler = tools[name][0]
            try:
                result = handler(conn, call_args)
            except Refused as exc:
                write({"jsonrpc": "2.0", "id": request_id, "result": {
                    "content": [{"type": "text", "text": str(exc)}],
                    "isError": True,
                }})
                continue
            except Exception as exc:  # pragma: no cover - defensive
                write({"jsonrpc": "2.0", "id": request_id, "error": {
                    "code": -32603, "message": "%s: %s" % (type(exc).__name__, exc)}})
                continue
            changed = isinstance(result, dict) and result.pop("_tools_changed", False)
            write({"jsonrpc": "2.0", "id": request_id, "result": {
                "content": [{"type": "text", "text": json.dumps(result)}],
                "isError": False,
            }})
            if changed:
                write({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        elif method == "ping":
            write({"jsonrpc": "2.0", "id": request_id, "result": {}})
        elif request_id is not None:
            write({"jsonrpc": "2.0", "id": request_id, "error": {
                "code": -32601, "message": "unsupported method %r" % method}})


if __name__ == "__main__":
    main()
