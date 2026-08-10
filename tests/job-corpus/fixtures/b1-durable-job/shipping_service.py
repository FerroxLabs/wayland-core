#!/usr/bin/env python3
"""B-1 fixture: the shipping service the job must talk to.

This process is the INDEPENDENT OBSERVER for row B-1. It is not the program
under test and it shares no code with it. Everything it records is a fact about
the world: an HTTP request that really arrived.

Two endpoints, and the difference between them is the whole point:

  POST /register            -- AN EFFECT. Books a shipment and bills the
                               customer. There is NO de-duplication: two POSTs
                               for the same order create two shipments.
  GET  /shipments?order_id= -- NOT an effect. Read-only lookup so a job that
                               was interrupted can find out whether a shipment
                               it may have booked exists, instead of guessing.

The ledger is append-only, fsynced per line, and lives outside the workspace
the job under test is given. Duplicate or lost effects are therefore visible
even if the job rewrites every file it owns.

Interruption: --kill-at K --kill-variant {before,after} makes the service kill
the job's whole process tree at the K-th effect, either after the response has
been written (the job knows it succeeded, then dies) or before any response is
written (the shipment exists but the job never learned that). Both are real
crash shapes and the harness sweeps every K.

Usage:
  python3 shipping_service.py --port 8731 --ledger /path/ledger.jsonl \
      [--kill-at 3 --kill-variant before --pid-file /path/job.pid]
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "graders"))
from common import kill_tree  # noqa: E402

STATE_LOCK = threading.Lock()
SHIPMENTS = {}  # order_id -> list of tracking numbers, in booking order
SEQ = {"n": 0}
CFG = {}


def log(entry):
    with STATE_LOCK:
        SEQ["n"] += 1
        entry["seq"] = SEQ["n"]
        entry["ts"] = time.time()
        with open(CFG["ledger"], "a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")
            fh.flush()
            os.fsync(fh.fileno())
        return entry["seq"]


def effect_count():
    with STATE_LOCK:
        return sum(len(v) for v in SHIPMENTS.values())


def read_pid():
    path = CFG.get("pid_file")
    if not path:
        return None
    try:
        with open(path, "r", encoding="utf-8") as fh:
            return int(fh.read().strip())
    except Exception:
        return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # silence stderr spam
        pass

    def _json(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self._json(200, {"ok": True})
            return
        if parsed.path == "/shipments":
            qs = parse_qs(parsed.query)
            order_id = (qs.get("order_id") or [""])[0].strip().upper()
            with STATE_LOCK:
                found = list(SHIPMENTS.get(order_id, []))
            log(
                {
                    "kind": "lookup",
                    "method": "GET",
                    "path": self.path,
                    "order_id": order_id,
                    "found": found,
                }
            )
            self._json(200, {"order_id": order_id, "shipments": found})
            return
        self._json(404, {"error": "no such endpoint"})

    def do_POST(self):
        parsed = urlparse(self.path)
        if parsed.path != "/register":
            self._json(404, {"error": "no such endpoint"})
            return
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            log({"kind": "effect_rejected", "method": "POST", "raw": raw[:400]})
            self._json(400, {"error": "body must be JSON"})
            return
        order_id = str(body.get("order_id", "")).strip().upper()
        weight = body.get("weight_kg")
        if not order_id:
            log({"kind": "effect_rejected", "method": "POST", "raw": raw[:400]})
            self._json(400, {"error": "order_id required"})
            return

        # THE EFFECT. Booked before any response is written, exactly as a real
        # billing system would: the money is spent whether or not the caller
        # ever sees the reply.
        tracking = "TRK-" + secrets.token_hex(5).upper()
        with STATE_LOCK:
            SHIPMENTS.setdefault(order_id, []).append(tracking)
            nth = sum(len(v) for v in SHIPMENTS.values())

        kill_at = CFG.get("kill_at")
        variant = CFG.get("kill_variant")
        fires = kill_at is not None and nth == kill_at

        log(
            {
                "kind": "effect",
                "method": "POST",
                "path": self.path,
                "order_id": order_id,
                "weight_kg": weight,
                "tracking": tracking,
                "effect_index": nth,
                "kill": variant if fires else None,
                "responded": not (fires and variant == "before"),
            }
        )

        if fires and variant == "before":
            pid = read_pid()
            if pid:
                kill_tree(pid)
            time.sleep(1.0)
            try:
                self.close_connection = True
                self.connection.close()
            except Exception:
                pass
            return

        self._json(201, {"order_id": order_id, "tracking": tracking})

        if fires and variant == "after":
            pid = read_pid()
            if pid:
                time.sleep(0.20)  # let the reply reach the client
                kill_tree(pid)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8731)
    ap.add_argument("--ledger", required=True)
    ap.add_argument("--kill-at", type=int, default=None,
                    help="interrupt the job at the K-th booking (1-based)")
    ap.add_argument("--kill-variant", choices=["before", "after"], default="after")
    ap.add_argument("--pid-file", default=None,
                    help="file holding the pid of the job under test")
    args = ap.parse_args()

    CFG.update(
        ledger=os.path.abspath(args.ledger),
        kill_at=args.kill_at,
        kill_variant=args.kill_variant,
        pid_file=os.path.abspath(args.pid_file) if args.pid_file else None,
    )
    os.makedirs(os.path.dirname(CFG["ledger"]) or ".", exist_ok=True)
    # Never truncate: the ledger spans the crash and the resume.
    open(CFG["ledger"], "a", encoding="utf-8").close()

    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print("shipping service on http://127.0.0.1:%d ledger=%s" % (args.port, CFG["ledger"]),
          flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
