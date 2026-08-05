#!/usr/bin/env python3
"""Loopback OpenAI-compatible provider for the headless-keyring lane.

Answers POST /v1/chat/completions with a single scripted assistant turn, both
non-streaming and SSE. No credential of any kind is authenticated; the only
key-shaped value involved is a synthetic literal supplied by the caller.

Its ONLY job is to let a turn complete, so that "did wayland-core start AND
complete a real turn" can be separated from "did wayland-core merely start".
Every request is logged to the path in $MOCK_LOG so the caller can prove the
product actually reached a provider rather than short-circuiting.
"""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("MOCK_LOG", "/tmp/mock_provider.log")
REPLY = os.environ.get("MOCK_REPLY", "HEADLESS_TURN_OK")


def log(msg):
    with open(LOG, "a") as f:
        f.write(f"{time.time():.3f} {msg}\n")
        f.flush()


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
        log(f"POST {self.path} stream={body.get('stream')} model={body.get('model')} "
            f"msgs={len(body.get('messages') or [])}")
        if body.get("stream"):
            self._sse()
        else:
            self._json()

    def _json(self):
        payload = json.dumps({
            "id": "chatcmpl-headless", "object": "chat.completion",
            "created": int(time.time()), "model": "mock-model",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": REPLY}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
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

        def chunk(delta, finish=None, usage=None):
            d = {"id": "chatcmpl-headless", "object": "chat.completion.chunk",
                 "created": int(time.time()), "model": "mock-model",
                 "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            if usage is not None:
                d["usage"] = usage
            self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
            self.wfile.flush()

        chunk({"role": "assistant", "content": ""})
        chunk({"content": REPLY})
        chunk({}, finish="stop",
              usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2})
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8399
    log(f"listening on {port}")
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
