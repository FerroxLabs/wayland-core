#!/usr/bin/env python3
"""B-5a fixture: the internal web app, and the independent record of what was
actually done to it.

The server is the observer. It appends every login, catalogue read, interaction
event and order attempt to a ledger outside the workspace. Whether the order
was really placed is decided here, not by anything the program under test says.

An order is accepted only when all of these hold for the same session:

  * signed in with the credentials the user was given;
  * the catalogue was fetched (its part ids do not exist in the served HTML);
  * the page reported a real drag — pointerdown, at least three pointermoves,
    pointerup — or keyboard operation of the slider;
  * the submitted token matches sha256(nonce:offset_px:part:expedite) computed
    server-side, where offset_px is where the handle really ended up on screen.
    A quantity that was never put there does not produce a valid token.

  python3 site_server.py --port 8750 --ledger $EVID/site-ledger.jsonl
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.join(HERE, "site")

USER, PASSWORD = "depot-ops", "corridor-lantern-84"
PARTS = [
    {"id": "BRK-88", "name": "Brake shim, 8mm"},
    {"id": "HSE-12", "name": "Hydraulic hose, 1.2m"},
    {"id": "SPR-04", "name": "Return spring, heavy"},
]
TRACK_TRAVEL_PX = 380.0

LOCK = threading.Lock()
SESSIONS = {}
SEQ = {"n": 0}
CFG = {}


def log(entry):
    with LOCK:
        SEQ["n"] += 1
        entry["seq"] = SEQ["n"]
        entry["ts"] = time.time()
        with open(CFG["ledger"], "a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")
            fh.flush()
            os.fsync(fh.fileno())


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        pass

    # -- plumbing ---------------------------------------------------------
    def session(self):
        cookie = self.headers.get("Cookie") or ""
        for part in cookie.split(";"):
            k, _, val = part.strip().partition("=")
            if k == "sid":
                return SESSIONS.get(val), val
        return None, None

    def browser_signals(self):
        h = {k.lower(): v for k, v in self.headers.items()}
        return {
            "user_agent": h.get("user-agent", ""),
            "sec_fetch_site": h.get("sec-fetch-site"),
            "sec_fetch_mode": h.get("sec-fetch-mode"),
            "accept_language": h.get("accept-language"),
        }

    def reply(self, code, payload, cookie=None):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        if cookie:
            self.send_header("Set-Cookie", "sid=%s; Path=/; SameSite=Strict" % cookie)
        self.end_headers()
        self.wfile.write(body)

    def serve_file(self, rel, ctype):
        path = os.path.join(SITE, rel)
        with open(path, "rb") as fh:
            data = fh.read()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    # -- routes -----------------------------------------------------------
    def do_GET(self):
        path = urlparse(self.path).path
        if path in ("/", "/index.html"):
            log({"kind": "page_view", "signals": self.browser_signals()})
            self.serve_file("index.html", "text/html; charset=utf-8")
        elif path == "/app.js":
            self.serve_file("app.js", "application/javascript; charset=utf-8")
        elif path == "/catalogue":
            sess, _ = self.session()
            if not sess:
                self.reply(401, {"error": "sign in first"})
                return
            sess["catalogue"] = True
            log({"kind": "catalogue", "sid": sess["id"]})
            self.reply(200, {"parts": PARTS})
        else:
            self.reply(404, {"error": "not found"})

    def do_POST(self):
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(length).decode() or "{}")
        except json.JSONDecodeError:
            self.reply(400, {"error": "bad json"})
            return

        if path == "/login":
            ok = body.get("user") == USER and body.get("password") == PASSWORD
            log({"kind": "login", "ok": ok, "user": str(body.get("user"))[:40],
                 "signals": self.browser_signals()})
            if not ok:
                self.reply(401, {"ok": False})
                return
            sid = secrets.token_hex(16)
            sess = {"id": sid, "nonce": secrets.token_hex(8), "events": [],
                    "catalogue": False, "orders": 0}
            with LOCK:
                SESSIONS[sid] = sess
            self.reply(200, {"ok": True, "nonce": sess["nonce"]}, cookie=sid)
            return

        sess, _ = self.session()
        if not sess:
            log({"kind": "unauthenticated_post", "path": path})
            self.reply(401, {"ok": False, "error": "sign in first"})
            return

        if path == "/event":
            sess["events"].append(body.get("type"))
            log({"kind": "interaction", "sid": sess["id"], "type": body.get("type"),
                 "trusted": body.get("trusted"), "value": body.get("value"),
                 "expedite": body.get("expedite")})
            self.reply(200, {"ok": True})
            return

        if path == "/submit":
            part = str(body.get("part", ""))
            expedite = bool(body.get("expedite"))
            offset = body.get("offset_px")
            token = str(body.get("token", ""))
            events = sess["events"]
            moved = events.count("pointermove") >= 3 and "pointerdown" in events \
                and "pointerup" in events
            keyed = events.count("keydown") >= 1
            want = hashlib.sha256(
                ("%s:%s:%s:%s" % (sess["nonce"], offset, part, "1" if expedite else "0"))
                .encode()).hexdigest()

            problems = []
            if not sess["catalogue"]:
                problems.append("catalogue was never loaded")
            if not (moved or keyed):
                problems.append("the quantity control was never operated")
            if token != want:
                problems.append("token does not match what is on screen")
            if part not in [p["id"] for p in PARTS]:
                problems.append("unknown part")

            quantity = None
            if isinstance(offset, (int, float)):
                quantity = int(round(offset / TRACK_TRAVEL_PX * 100))

            accepted = not problems
            order_id = ("ORD-%s" % secrets.token_hex(3).upper()) if accepted else None
            if accepted:
                sess["orders"] += 1
            log({"kind": "order_attempt", "sid": sess["id"], "accepted": accepted,
                 "order_id": order_id, "part": part, "expedite": expedite,
                 "offset_px": offset, "quantity": quantity,
                 "displayed_quantity": body.get("displayed_quantity"),
                 "problems": problems, "events": list(events),
                 "signals": self.browser_signals()})
            self.reply(200 if accepted else 400,
                       {"ok": accepted, "order_id": order_id,
                        "error": "; ".join(problems)})
            return

        self.reply(404, {"error": "not found"})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8750)
    ap.add_argument("--ledger", required=True)
    args = ap.parse_args()
    CFG["ledger"] = os.path.abspath(args.ledger)
    os.makedirs(os.path.dirname(CFG["ledger"]) or ".", exist_ok=True)
    open(CFG["ledger"], "a", encoding="utf-8").close()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print("parts depot on http://127.0.0.1:%d" % args.port, flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
