#!/usr/bin/env python3
"""B-2 fixture: a real provider outage, injected on the wire.

The program under test is pointed at this proxy instead of at the model
provider. The proxy forwards everything upstream verbatim until the trigger
fires, then makes the provider genuinely unusable: connection resets, 503s, or
a hang, for as long as the harness says.

The outage is timed off the WORK, not off a stopwatch: `--trigger-path` names a
file in the user's project, and the outage begins with the first request after
that file exists. Point it at the artifact the first half of the job produces
and the failure always lands mid-task, in the same place, on every platform.

Credential hygiene (INV-1): request and response BODIES are never written
anywhere, and Authorization / x-api-key / api-key / cookie headers are dropped
from the ledger. Only method, path, status, byte counts and timings are logged.

Usage:
  python3 provider_proxy.py --port 8788 \
      --upstream https://api.example-provider.com \
      --ledger $EVID/proxy-ledger.jsonl \
      --trigger-path $WS/ledger/schema.json \
      --fault reset --fault-requests 6
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import socket
import ssl
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

CFG = {}
LOCK = threading.Lock()
STATE = {"seq": 0, "latched": False, "faulted": 0}

REDACT = {"authorization", "x-api-key", "api-key", "cookie", "set-cookie",
          "proxy-authorization", "x-goog-api-key", "openai-organization"}
HOP = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
       "te", "trailers", "transfer-encoding", "upgrade", "host", "content-length"}


def log(entry):
    with LOCK:
        STATE["seq"] += 1
        entry["seq"] = STATE["seq"]
        entry["ts"] = time.time()
        with open(CFG["ledger"], "a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")
            fh.flush()
            os.fsync(fh.fileno())


def fault_active():
    """Latch once the trigger file appears; stay faulty for N requests."""
    with LOCK:
        if not STATE["latched"]:
            tp = CFG.get("trigger_path")
            if tp and os.path.exists(tp):
                STATE["latched"] = True
            elif CFG.get("fault_from_start"):
                STATE["latched"] = True
        if not STATE["latched"]:
            return False
        budget = CFG.get("fault_requests")
        if budget is not None and STATE["faulted"] >= budget:
            return False
        STATE["faulted"] += 1
        return True


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "b2-fault-proxy"

    def log_message(self, fmt, *args):
        pass

    def _relay(self, method):
        started = time.time()
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""

        if fault_active():
            mode = CFG["fault"]
            log({"kind": "fault", "method": method, "path": self.path,
                 "fault": mode, "req_bytes": len(body)})
            if mode == "reset":
                try:
                    self.connection.setsockopt(
                        socket.SOL_SOCKET, socket.SO_LINGER,
                        b"\x01\x00\x00\x00\x00\x00\x00\x00")
                    self.connection.close()
                except Exception:
                    pass
                self.close_connection = True
                return
            if mode == "timeout":
                time.sleep(CFG.get("hang_seconds", 120))
                self.close_connection = True
                return
            payload = json.dumps({"error": {"type": "overloaded_error",
                                            "message": "upstream is unavailable"}}).encode()
            self.send_response(503)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        up = CFG["upstream"]
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        try:
            if up.scheme == "https":
                conn = http.client.HTTPSConnection(
                    up.hostname, up.port or 443, timeout=CFG["upstream_timeout"],
                    context=ssl.create_default_context())
            else:
                conn = http.client.HTTPConnection(
                    up.hostname, up.port or 80, timeout=CFG["upstream_timeout"])
            path = (up.path.rstrip("/") + self.path) if up.path.strip("/") else self.path
            conn.request(method, path, body=body, headers=headers)
            resp = conn.getresponse()
        except Exception as exc:
            log({"kind": "upstream_error", "method": method, "path": self.path,
                 "error": type(exc).__name__})
            self.send_response(502)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        self.send_response(resp.status)
        out_headers = [(k, v) for k, v in resp.getheaders() if k.lower() not in HOP]
        for k, v in out_headers:
            self.send_header(k, v)
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        total = 0
        try:
            while True:
                chunk = resp.read(8192)
                if not chunk:
                    break
                total += len(chunk)
                self.wfile.write(b"%X\r\n" % len(chunk) + chunk + b"\r\n")
                self.wfile.flush()
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
        except Exception:
            self.close_connection = True
        finally:
            try:
                conn.close()
            except Exception:
                pass

        log({"kind": "relay", "method": method, "path": self.path,
             "status": resp.status, "req_bytes": len(body), "resp_bytes": total,
             "duration_s": round(time.time() - started, 3),
             "headers_seen": sorted(k.lower() for k in self.headers.keys()
                                    if k.lower() not in REDACT)})

    def do_GET(self):
        self._relay("GET")

    def do_POST(self):
        self._relay("POST")

    def do_PUT(self):
        self._relay("PUT")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--upstream", required=True)
    ap.add_argument("--ledger", required=True)
    ap.add_argument("--trigger-path", default=None,
                    help="outage begins with the first request after this file exists")
    ap.add_argument("--fault-from-start", action="store_true")
    ap.add_argument("--fault", choices=["reset", "http503", "timeout"], default="reset")
    ap.add_argument("--fault-requests", type=int, default=None,
                    help="number of requests to break before healing (default: forever)")
    ap.add_argument("--hang-seconds", type=float, default=120.0)
    ap.add_argument("--upstream-timeout", type=float, default=600.0)
    args = ap.parse_args()

    CFG.update(
        upstream=urlparse(args.upstream),
        ledger=os.path.abspath(args.ledger),
        trigger_path=os.path.abspath(args.trigger_path) if args.trigger_path else None,
        fault_from_start=args.fault_from_start,
        fault=args.fault,
        fault_requests=args.fault_requests,
        hang_seconds=args.hang_seconds,
        upstream_timeout=args.upstream_timeout,
    )
    os.makedirs(os.path.dirname(CFG["ledger"]) or ".", exist_ok=True)
    open(CFG["ledger"], "a", encoding="utf-8").close()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print("fault proxy on http://127.0.0.1:%d -> %s" % (args.port, args.upstream), flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
