"""A-11 — do a real job in an external system, through MCP, verified from outside.

The warehouse is a stateful stdio MCP server with an authorisation model.  The
job spans four dependent calls and one refusal, and the correct outcome is a set
of rows in a SQLite database that only real tool calls can produce.  Nothing in
the product's reply is read: `keys/a11_verify.py` opens the database.

The fixture was built to catch the three defects that made MCP tools uncallable,
so this row fails, by name, on each of them:

* **whole-query substring matching** — the catalogue holds `reserve`,
  `inventory_reserve` and `inventory_reserve_bulk`.  Only the middle one works.
  Resolving the name loosely moves no stock, and the stock is what is graded.
* **bulk registration that never refreshes the catalogue** —
  `inventory_audit_export` does not exist at session start.  It is registered
  after the first despatch and the server sends `notifications/tools/list_changed`.
  A client that lists tools once cannot finish the job, and fails on exactly one
  check: the missing export file.
* **no callability signal** — `inventory_purge` is advertised and always
  refuses; every mutating tool refuses for an unauthorised actor.  Being listed
  is not being callable.

WAREHOUSE_TOKEN is a fixture string with no meaning outside this test.  It is
not a credential and must not be replaced with one.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _provision as P  # noqa: E402
from harness.result import FAIL, UNPROVEN, Check  # noqa: E402

ROW_ID = "A-11"
TIER = "A"
TITLE = "drive an external system through MCP"
FIXTURE = "fixtures/a11_mcp"
KEY = "keys/a11.key.json"

#: The only thing the job should write into the working directory is the export
#: the late-registered tool produces. The warehouse database lives outside the
#: workspace, because it is the external system, not the user's files.
DECLARED_SCOPE = ["audit-export.json"]

TIMEOUT = 1500

#: Fixture value. Not a credential; it exists so the server can refuse when it
#: is absent, which is the "the agent was never really asked" case.
FIXTURE_TOKEN = "test-token-not-a-real-credential"

PROMPT = (
    "Read TASK.md in this directory and do the job it describes using the "
    "warehouse MCP server that is connected. When you are finished, stop."
)


def run(ctx):
    server = os.path.join(ctx.workspace, "warehouse_mcp.py")
    if not os.path.isfile(server):
        ctx.add_check(Check(ROW_ID + ".fixture", UNPROVEN, "the warehouse server is not in the fixture"))
        return

    db = os.path.join(ctx.artifact_dir, "warehouse.db")
    export = os.path.join(ctx.workspace, "audit-export.json")

    mcp = {
        "warehouse": {
            "transport": "stdio",
            "command": sys.executable,
            "args": [server, "--db", db],
            "env": {"WAREHOUSE_TOKEN": FIXTURE_TOKEN},
        }
    }
    try:
        prov = P.provision(ctx.artifact_dir, mcp_servers=mcp)
    except P.NotProvisioned as exc:
        ctx.add_check(P.unprovisioned_check(ROW_ID, exc))
        return
    ctx.record.world["provisioning"] = prov.describe()
    ctx.record.world["a11_mcp_server"] = {
        "transport": "stdio",
        "command": sys.executable,
        "args": [server, "--db", db],
        "token_variable": "WAREHOUSE_TOKEN",
    }

    rec = P.drive(ctx, PROMPT, prov, timeout=TIMEOUT)
    ctx.add_check(P.session_ran_check(ctx, ROW_ID, [rec]))

    # If the server was never started at all, nothing was asked of the product
    # and a FAIL would be a statement about the harness.
    if not os.path.exists(db):
        ctx.add_check(
            Check(
                ROW_ID + ".stock-moved",
                UNPROVEN,
                "the warehouse database was never created, so the MCP server never "
                "started and the job was never actually put to the product",
                {"db": db, "product_output_tail": P.reply_text(ctx, rec)[-3000:]},
            )
        )
        return

    report, raw, _code = P.run_grader(
        [
            os.path.join(P.CORPUS_ROOT, "keys", "a11_verify.py"),
            "--db", db,
            "--export", export,
            "--json", os.path.join(ctx.artifact_dir, "a11_verify.json"),
        ],
        timeout=300,
    )
    ctx.add_check(
        P.grader_check(
            ROW_ID + ".stock-moved",
            report,
            raw,
            "the warehouse really moved: both lines were held and despatched, the "
            "line that could not be filled was left alone, every change carries the "
            "authorised actor and the order reference, and the audit trail can be "
            "handed on",
            "keys/a11_verify.py",
        )
    )

    observed = (report or {}).get("observed") or {}

    # The three defects, each named on its own so a failure says which one.
    audit = observed.get("audit") or []
    ctx.expect(
        not any(a.get("tool") == "reserve" and a.get("outcome") == "ok" for a in audit),
        ROW_ID + ".exact-tool-called",
        "the job used the tool that exists rather than one whose name merely looks "
        "similar",
        "a retired look-alike tool was called as if it were the real one, which "
        "moves no stock: %d call(s) to `reserve`"
        % sum(1 for a in audit if a.get("tool") == "reserve"),
        {"deprecated_calls": observed.get("called_deprecated_reserve")},
    )
    ctx.expect(
        os.path.exists(export),
        ROW_ID + ".late-tool-reached",
        "the tool that only appears part way through the session was found and used, "
        "so the audit trail could be handed to someone else",
        "audit-export.json was never written. That file can only be produced by a "
        "tool which does not exist when the session starts and appears after the "
        "first despatch, so a client that lists its tools once and never refreshes "
        "cannot finish this job",
        {"export": export},
    )
    ctx.expect(
        not any(a.get("tool") == "inventory_purge" and a.get("outcome") == "ok" for a in audit),
        ROW_ID + ".listed-is-not-callable",
        "a tool that is advertised but always refuses did not succeed, so being "
        "listed was not mistaken for being callable",
        "inventory_purge reported success, and it must never succeed",
        {"audit": audit[:40]},
    )

    try:
        conn = sqlite3.connect(db)
        rows = list(conn.execute("SELECT sku, on_hand, reserved FROM stock ORDER BY sku"))
        conn.close()
        ctx.record.world["a11_final_stock"] = [
            {"sku": r[0], "on_hand": r[1], "reserved": r[2]} for r in rows
        ]
    except sqlite3.Error as exc:
        ctx.record.world["a11_final_stock_error"] = str(exc)
