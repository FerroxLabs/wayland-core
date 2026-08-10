"""A-11 — drive a real external system through MCP, and be graded from outside it.

The warehouse server is a stateful SQLite-backed MCP server launched by the
PRODUCT over stdio, from the product's own configuration.  The job spans four
dependent calls and one refusal, so no single round trip finishes it, and the
correct outcome is a set of rows in a database rather than a returned string.
``keys/a11_verify.py`` opens that database directly: it never reads the agent's
reply, never asks the server what it did, and never trusts a tool result.

The fixture is built to catch the three defects that once made every MCP tool
in this product uncallable:

  * the catalogue holds ``reserve``, ``inventory_reserve`` and
    ``inventory_reserve_bulk`` and only the middle one works, so a loose
    substring match moves no stock and the database says so;
  * ``inventory_audit_export`` does not exist at session start — it is
    registered on the first successful despatch, followed by
    ``notifications/tools/list_changed`` — so a client that lists tools once
    and never again cannot write the export file, and fails on exactly that;
  * every mutating tool refuses outright for an unauthorised actor, and
    ``inventory_purge`` always refuses. Being listed is not being callable.

``deferred = false`` is set deliberately: it puts the catalogue in the system
prompt at startup, which is the configuration in which the late-registered
export tool has to arrive by ``list_changed`` rather than by a lookup at call
time.  That is the defect this row exists to detect.

WAREHOUSE_TOKEN is a fixture value with no meaning outside this test.  It is
not a real credential and must never be replaced with one.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _agrade as G  # noqa: E402
import _common as C  # noqa: E402

from harness.result import UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402

ROW_ID = "A-11"
TIER = "A"
TITLE = "drive an external system through MCP"
FIXTURE = "fixtures/a11_mcp"
KEY = "keys/a11.key.json"
#: The only thing the job was asked to write into the working directory. The
#: warehouse itself lives outside the workspace, where the agent cannot edit it
#: with a text editor instead of calling the tools.
DECLARED_SCOPE = ["audit-export.json"]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
TEST_COMMAND = None  # keys/a11_verify.py reads the warehouse database
TIMEOUT = 2400

FIXTURE_DIR = os.path.join(C.FIXTURES, "a11_mcp")
SERVER = os.path.join(FIXTURE_DIR, "warehouse_mcp.py")
VERIFIER = os.path.join(C.KEYS, "a11_verify.py")

#: Not a credential. A fixture constant, checked into the repository on
#: purpose, so that a run cannot be made to depend on a real secret.
WAREHOUSE_TOKEN = "warehouse-fixture-token-not-a-real-credential"

EXPORT_NAME = "audit-export.json"


def main(binary: str, artifact_dir: str):
    cred = C.credential()
    workspace = G.copy_fixture_repo(
        FIXTURE_DIR, os.path.join(artifact_dir, "ws"), include=["TASK.md"]
    )

    if cred is None:
        runner = RowRunner(ROW_ID, binary, workspace, artifact_dir, tier=TIER, title=TITLE)
        runner.record.add_check(Check(ROW_ID + ".not-run", UNPROVEN, C.MISSING_CREDENTIAL))
        return runner.record

    with RowContext(
        row_id=ROW_ID,
        binary=binary,
        artifact_dir=artifact_dir,
        workspace=workspace,
        declared_scope=DECLARED_SCOPE,
        scope_ignore_extra=SCOPE_IGNORE,
        test_command=None,
        timeout=TIMEOUT,
        tier=TIER,
        title=TITLE,
        leak_upstream=C.upstream_base_url(cred),
        key_path=os.path.join(C.KEYS, "a11.key.json"),
    ) as ctx:
        try:
            _run(ctx, cred)
        finally:
            echoed = C.scan_product_output_for_secret(ctx, cred)
            C.grade_credential_hygiene(ctx, ROW_ID, cred, echoed)
            C.note_product_detritus(ctx, ROW_ID)
    return ctx.record


def server_answers_on_this_host(db_path: str):
    """Prove the warehouse server starts and lists tools BEFORE the product runs.

    Without this, a server that cannot start on the host would arrive as a
    product failure. The key says that case is UNPROVEN, and this is what makes
    the distinction observable rather than assumed.
    """
    env = dict(os.environ)
    env.pop("API_KEY", None)
    env.pop("FLUX_API_KEY", None)
    env["WAREHOUSE_TOKEN"] = WAREHOUSE_TOKEN
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    proc = None
    try:
        proc = subprocess.Popen(
            [G.PY, SERVER, "--db", db_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=env,
            text=True,
            bufsize=1,
        )
        tools = []
        for seq, (method, params) in enumerate(
            (("initialize", {}), ("tools/list", {})), start=1
        ):
            proc.stdin.write(
                json.dumps({"jsonrpc": "2.0", "id": seq, "method": method, "params": params})
                + "\n"
            )
            proc.stdin.flush()
            deadline = time.time() + 20
            while time.time() < deadline:
                line = proc.stdout.readline()
                if not line:
                    return False, "the warehouse server closed the connection", []
                message = json.loads(line)
                if "id" not in message:
                    continue
                if method == "tools/list":
                    tools = [
                        t.get("name")
                        for t in (message.get("result") or {}).get("tools", [])
                    ]
                break
            else:
                return False, "the warehouse server did not answer %s in 20s" % method, []
        if not tools:
            return False, "the warehouse server advertised no tools at all", []
        return True, "advertises %d tool(s) at session start" % len(tools), tools
    except (OSError, ValueError) as exc:
        return False, "the warehouse server could not be started: %s" % exc, []
    finally:
        if proc is not None:
            try:
                proc.stdin.close()
            except OSError:
                pass
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()


def _write_mcp_config(ctx: RowContext, db_path: str) -> str:
    """Add the warehouse to the product's OWN configuration, and nothing else.

    Appended rather than written, so an `auth add` that also wrote config is
    not silently discarded by this row.
    """
    home = ctx.runner.base_env.get("WAYLAND_HOME")
    if not home:
        raise RuntimeError("the runner set no WAYLAND_HOME, so the product has no config to edit")
    os.makedirs(home, exist_ok=True)
    path = os.path.join(home, "config.toml")
    block = (
        "\n[mcp.servers.warehouse]\n"
        'transport = "stdio"\n'
        "command = %s\n"
        "args = [%s, %s, %s]\n"
        "deferred = false\n"
        "env = { WAREHOUSE_TOKEN = %s }\n"
        % (
            json.dumps(G.PY),
            json.dumps(SERVER),
            json.dumps("--db"),
            json.dumps(db_path),
            json.dumps(WAREHOUSE_TOKEN),
        )
    )
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(block)
    return path


def _run(ctx: RowContext, cred: C.Credential) -> None:
    C.isolate_provider_env(ctx)
    C.clear_prewritten_config(ctx)
    C.authenticate(ctx, cred)

    db_path = os.path.join(ctx.artifact_dir, "warehouse.sqlite3")
    probe_db = os.path.join(ctx.artifact_dir, "warehouse-preflight.sqlite3")
    alive, detail, tools = server_answers_on_this_host(probe_db)
    ctx.record.world["a11_server_preflight"] = {"alive": alive, "detail": detail, "tools": tools}
    if not alive:
        ctx.unproven(
            ROW_ID + ".the-warehouse-really-moved",
            "the warehouse MCP server would not run on this host (%s), so the "
            "agent was never actually asked to do anything and no verdict about "
            "the product can be drawn" % detail,
        )
        ctx.unproven(
            ROW_ID + ".the-audit-export-was-written",
            "the warehouse MCP server would not run on this host, so the "
            "late-registered export tool never existed to be found",
        )
        return

    config_path = _write_mcp_config(ctx, db_path)
    G.note(
        ctx,
        ROW_ID + ".server-configuration",
        "the warehouse was declared to the product as a stdio MCP server with "
        "deferred = false; it advertises %d tool(s) before the job starts and "
        "registers inventory_audit_export only after the first despatch"
        % len(tools),
        {"config": config_path, "tools_at_start": tools, "db": db_path},
    )

    job = ctx.run(
        C.product_argv(cred, G.task_prompt(FIXTURE_DIR), max_turns=80,
                       base_url=ctx.provider_base_url),
        extra_env=C.product_env(cred),
        timeout=TIMEOUT,
    )

    export_path = os.path.join(ctx.workspace, EXPORT_NAME)
    json_path = os.path.join(ctx.artifact_dir, "a11-verify.json")
    rc, report, raw = G.run_grader(
        [G.PY, VERIFIER, "--db", db_path, "--export", export_path, "--json", json_path],
        cwd=C.KEYS,
        json_path=json_path,
        timeout=900,
    )
    ctx.record.world["a11_verifier"] = {"exit": rc, "report": report}
    G.set_independent(
        ctx,
        [G.PY, "a11_verify.py", "--db", db_path, "--export", export_path],
        isinstance(report, dict) and report.get("verdict") == "PASS",
        "\n".join(str(r) for r in ((report or {}).get("reasons") or [])),
        ctx.workspace,
    )

    G.apply_verdict(
        ctx,
        ROW_ID + ".the-warehouse-really-moved",
        "the stock levels a warehouse manager would look at are exactly right: "
        "two lines held and despatched once each, the line that could not be "
        "filled left completely alone, and every change attributable to "
        "dispatch-bot and to order SO-88120",
        report,
        raw,
        "the warehouse verifier produced no readable verdict, so nothing is "
        "known about what, if anything, the job did to the real system",
    )

    # Named separately because it has one specific cause: a client that lists
    # tools once and never refreshes cannot ever write this file.
    ctx.expect(
        os.path.isfile(export_path),
        ROW_ID + ".the-audit-export-was-written",
        "audit-export.json exists — which is only possible if the client "
        "noticed inventory_audit_export appearing mid-session and called it",
        "audit-export.json was never written. That file can only be produced "
        "by inventory_audit_export, which does not exist when the session "
        "starts and only appears after the first despatch, so a client that "
        "never refreshes its tool catalogue cannot finish this job.",
        {"export_path": export_path, "job_exit": job.exit_code},
    )

    if isinstance(report, dict):
        G.note(
            ctx,
            ROW_ID + ".warehouse-state",
            "final stock %s; reservations %s"
            % (
                (report.get("observed") or {}).get("stock"),
                (report.get("observed") or {}).get("reservations"),
            ),
            {"observed": report.get("observed")},
        )
