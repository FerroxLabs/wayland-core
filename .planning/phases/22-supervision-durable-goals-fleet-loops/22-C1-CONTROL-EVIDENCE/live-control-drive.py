#!/usr/bin/env python3
"""F22-C1 live drive: host CONTROL of a durable Goal over the real JSON stream.

Drives the SHIPPED `wayland-core --json-stream` binary. Nothing here reaches
into the library: every command is a JSON line on the process's stdin and every
assertion is made against a JSON line on its stdout. That is the difference
between proving a host can control a Goal and proving a Rust function can.

## Why this script asserts the way it does

- It reads the live `session_id` out of the product's own `ready` event rather
  than assuming one. An env/config-derived guess would make `session_not_found`
  indistinguishable from a real refusal (LANE-BRIEF 3b-ii).
- It asserts on the CHAIN's reported state (`iterations_started` moving), not on
  "a snapshot came back". A producer that echoed the request would pass the
  weaker check.
- Every gate is falsified in the same run: the stale-cursor advance must be
  REFUSED, and the script fails if it is accepted.
- Exit status is written to a status file by the caller; this script's own
  verdict is printed as an explicit final line so a truncated capture cannot
  read as a pass.
"""

import json
import os
import subprocess
import sys
import tempfile
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/wayland-core"
NONCE = sys.argv[2] if len(sys.argv) > 2 else "22c1ctl"
GOAL = f"g-{NONCE}"

failures = []


def check(label, ok, detail=""):
    print(f"{'PASS' if ok else 'FAIL'}  {label}" + (f"  :: {detail}" if detail else ""))
    if not ok:
        failures.append(label)
    return ok


workdir = tempfile.mkdtemp(prefix=f"wl-{NONCE}-")
env = dict(os.environ)
env["WAYLAND_CONFIG_PATH"] = os.path.join(workdir, "config.toml")
with open(env["WAYLAND_CONFIG_PATH"], "w") as handle:
    handle.write("[provider]\nname = \"anthropic\"\n")

proc = subprocess.Popen(
    [BIN, "--json-stream"],
    cwd=workdir,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
    env=env,
)


def send(obj):
    line = json.dumps(obj)
    print(f">>> {line}")
    proc.stdin.write(line + "\n")
    proc.stdin.flush()


def read_until(pred, budget=30.0):
    """Read stdout lines until `pred` matches. Returns the event or None."""
    deadline = time.time() + budget
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        etype = event.get("type")
        if etype in ("goal_snapshot", "goal_transition", "goal_control_refused", "ready"):
            print(f"<<< {json.dumps(event)[:400]}")
        if pred(event):
            return event
    return None


try:
    ready = read_until(lambda e: e.get("type") == "ready")
    if not check("ready received", ready is not None):
        raise SystemExit(1)

    # Read the session id back from the PRODUCT, never assume it.
    session_id = ready.get("session_id")
    check("ready carries a session_id", bool(session_id), f"session_id={session_id}")

    caps = (ready.get("capabilities") or {})
    contract = ready.get("contract") or {}
    print(f"### contract={json.dumps(contract)[:300]}")

    # ---------------------------------------------------------------- open
    send({
        "type": "goal_open", "goal_version": 1, "request_id": "live-open",
        "session_id": session_id, "goal_id": GOAL,
        "objective": "F22-C1 live control drive", "iterations": 4,
        "strategy": "fleet", "max_tokens": 10000,
    })
    snap = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    if not check("goal_open answered", snap is not None):
        raise SystemExit(1)
    if not check(
        "goal_open ACCEPTED (not refused)",
        snap.get("type") == "goal_snapshot",
        json.dumps(snap)[:200],
    ):
        raise SystemExit(1)
    check("the opened Goal is the one requested", snap.get("goal_id") == GOAL)
    goal = snap.get("goal") or {}
    check(
        "lifecycle is opened",
        (goal.get("lifecycle") or {}).get("state") == "opened",
        json.dumps(goal.get("lifecycle")),
    )
    check(
        "iterations_started starts at 0",
        goal.get("iterations_started") == 0,
        str(goal.get("iterations_started")),
    )
    # The envelope is the INTERSECTION with the session's parent, not the ask.
    print(f"### recorded authority={json.dumps(goal.get('authority'))[:300]}")
    cursor = goal.get("cursor")
    check("snapshot carries a cursor", cursor is not None, json.dumps(cursor))
    stale_cursor = dict(cursor)

    # ------------------------------------------------------------- advance
    send({
        "type": "goal_advance", "goal_version": 1, "request_id": "live-adv-1",
        "session_id": session_id, "goal_id": GOAL, "cursor": cursor,
    })
    adv = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    if not check("goal_advance answered", adv is not None):
        raise SystemExit(1)
    check(
        "goal_advance ACCEPTED at the current cursor",
        adv.get("type") == "goal_snapshot",
        json.dumps(adv)[:200],
    )
    goal2 = adv.get("goal") or {}
    # THE control assertion: the Goal actually changed state.
    check(
        "GOAL STATE CHANGED: iterations_started 0 -> 1",
        goal2.get("iterations_started") == 1,
        str(goal2.get("iterations_started")),
    )
    check(
        "GOAL STATE CHANGED: lifecycle opened -> running",
        (goal2.get("lifecycle") or {}).get("state") == "running",
        json.dumps(goal2.get("lifecycle")),
    )

    # -------------------------------------------- KNOWN-NEGATIVE: stale cursor
    send({
        "type": "goal_advance", "goal_version": 1, "request_id": "live-adv-stale",
        "session_id": session_id, "goal_id": GOAL, "cursor": stale_cursor,
    })
    stale = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    if not check("stale advance answered", stale is not None):
        raise SystemExit(1)
    check(
        "KNOWN-NEGATIVE: stale cursor is REFUSED, not applied",
        stale.get("type") == "goal_control_refused",
        json.dumps(stale)[:250],
    )
    check(
        "refusal reason is cursor_stale",
        stale.get("reason") == "cursor_stale",
        str(stale.get("reason")),
    )

    # ------------------------------------ KNOWN-NEGATIVE: wrong session id
    send({
        "type": "goal_resync", "goal_version": 1, "request_id": "live-badsession",
        "session_id": "not-the-live-session",
    })
    bad = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    check(
        "KNOWN-NEGATIVE: a foreign session_id is refused",
        bad is not None and bad.get("type") == "goal_control_refused"
        and bad.get("reason") == "session_not_found",
        json.dumps(bad)[:200] if bad else "no answer",
    )

    # --------------------------------------------------------------- cancel
    send({
        "type": "goal_resync", "goal_version": 1, "request_id": "live-resync",
        "session_id": session_id, "goal_id": GOAL,
    })
    cur = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    check("resync answered with a snapshot", cur is not None and cur.get("type") == "goal_snapshot")
    live_cursor = ((cur or {}).get("goal") or {}).get("cursor")

    send({
        "type": "goal_cancel", "goal_version": 1, "request_id": "live-cancel",
        "session_id": session_id, "goal_id": GOAL, "cursor": live_cursor,
    })
    cancelled = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    if not check("goal_cancel answered", cancelled is not None):
        raise SystemExit(1)
    check(
        "goal_cancel ACCEPTED",
        cancelled.get("type") == "goal_snapshot",
        json.dumps(cancelled)[:200],
    )
    lifecycle = ((cancelled.get("goal") or {}).get("lifecycle")) or {}
    check(
        "GOAL STATE CHANGED: lifecycle -> terminated/cancelled",
        lifecycle.get("state") == "terminated"
        and lifecycle.get("terminal") == "cancelled",
        json.dumps(lifecycle),
    )

    # ------------------------------ KNOWN-NEGATIVE: cancelling a dead Goal
    send({
        "type": "goal_cancel", "goal_version": 1, "request_id": "live-cancel-2",
        "session_id": session_id, "goal_id": GOAL, "cursor": live_cursor,
    })
    again = read_until(lambda e: e.get("type") in ("goal_snapshot", "goal_control_refused"))
    check(
        "KNOWN-NEGATIVE: a terminated Goal refuses a second cancel",
        again is not None and again.get("type") == "goal_control_refused",
        json.dumps(again)[:200] if again else "no answer",
    )

finally:
    try:
        proc.stdin.close()
    except Exception:
        pass
    try:
        proc.wait(timeout=10)
    except Exception:
        proc.kill()
    err = proc.stderr.read()
    if err.strip():
        print("### stderr tail:")
        for line in err.strip().splitlines()[-12:]:
            print(f"    {line}")

print()
print(f"LIVE-DRIVE-FAILURES={len(failures)}")
if failures:
    for name in failures:
        print(f"  FAILED: {name}")
print("LIVE-DRIVE-VERDICT=" + ("PASS" if not failures else "FAIL"))
print("LIVE-DRIVE-DONE")
sys.exit(1 if failures else 0)
