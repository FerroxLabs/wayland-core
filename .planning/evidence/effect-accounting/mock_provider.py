#!/usr/bin/env python3
"""Loopback OpenAI-compatible provider for the effect-accounting lane.

Derived from `.planning/evidence/headless-keyring/mock_provider.py`, with one
change that matters here: the reported `usage` is settable from the
environment, so the harness — not the product — decides how many tokens each
turn costs. That makes the meter deterministic and, crucially, INDEPENDENT of
the thing under test: token spend is read back from this server's own log,
never from wayland-core's stdout.

No credential of any kind is authenticated. The only key-shaped value involved
is a synthetic literal supplied by the caller.
"""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("MOCK_LOG", "/tmp/mock_provider.log")
REPLY = os.environ.get("MOCK_REPLY", "EFFECT_TURN_OK")
TOK_IN = int(os.environ.get("MOCK_TOKENS_IN", "1"))
TOK_OUT = int(os.environ.get("MOCK_TOKENS_OUT", "1"))


def log(msg):
    with open(LOG, "a") as f:
        f.write(f"{time.time():.3f} {msg}\n")
        f.flush()


def usage():
    return {
        "prompt_tokens": TOK_IN,
        "completion_tokens": TOK_OUT,
        "total_tokens": TOK_IN + TOK_OUT,
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):  # silence stderr noise
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except Exception:
            body = {}
        # BILLED is the meter line. One per provider round-trip, carrying the
        # exact usage this server will report, so the caller can sum spend
        # without asking the product what it thinks it spent.
        log(f"BILLED path={self.path} stream={body.get('stream')} "
            f"model={body.get('model')} msgs={len(body.get('messages') or [])} "
            f"tokens_in={TOK_IN} tokens_out={TOK_OUT}")
        if body.get("stream"):
            self._sse()
        else:
            self._json()

    def _json(self):
        payload = json.dumps({
            "id": "chatcmpl-effect", "object": "chat.completion",
            "created": int(time.time()), "model": "mock-model",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": REPLY}}],
            "usage": usage(),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _sse(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def chunk(delta, finish=None, u=None):
            d = {"id": "chatcmpl-effect", "object": "chat.completion.chunk",
                 "created": int(time.time()), "model": "mock-model",
                 "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            if u is not None:
                d["usage"] = u
            self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
            self.wfile.flush()

        chunk({"role": "assistant", "content": ""})
        chunk({"content": REPLY})
        chunk({}, finish="stop", u=usage())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8471
    log(f"listening on {port} tokens_in={TOK_IN} tokens_out={TOK_OUT}")
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
