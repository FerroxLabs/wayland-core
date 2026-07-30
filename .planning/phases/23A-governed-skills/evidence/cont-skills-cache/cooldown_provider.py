#!/usr/bin/env python3
"""Canned OpenAI-compatible endpoint for the CONT-* cache-economics cooldown proof.

ONE variable: $CANNED_MODE.

  fail -> every POST answers HTTP 503 with a retryable body. `ProviderError` is
          classified retryable (transient 5xx), `should_trip_breaker` counts it,
          and with `failure_threshold = 1` the breaker transitions to Open, which
          is the only state that emits the F05-TRUTH-3 runtime outcome triple.

  ok   -> every POST answers a valid SSE completion. No failure is recorded, the
          breaker stays Closed, and NO occurrence may be emitted.

Every request is appended to $CANNED_LOG. That log is the read-back that proves
the product reached THIS endpoint rather than the ANTHROPIC_API_KEY that
/root/.wayland/.env injects regardless of what the shell unsets
(LANE-BRIEF section 3b-ii). No credential is authenticated; the only key-shaped
value is a synthetic literal supplied by the caller.
"""

import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("CANNED_LOG", "/tmp/cont-skills-cache-canned.log")
PORT = int(os.environ.get("CANNED_PORT", "18944"))
MODE = os.environ.get("CANNED_MODE", "fail")
FINAL = os.environ.get("CANNED_FINAL", "COOLDOWN_PROOF_DONE")

STATE = {"n": 0}


def log(msg):
    with open(LOG, "a") as fh:
        fh.write(f"{time.time():.3f} {msg}\n")
        fh.flush()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except Exception:
            body = {}
        STATE["n"] += 1
        log(
            f"POST {self.path} req={STATE['n']} mode={MODE} "
            f"stream={body.get('stream')} model={body.get('model')}"
        )
        if MODE == "fail":
            self._fail()
        else:
            self._ok(body.get("stream"))

    def _fail(self):
        payload = json.dumps(
            {"error": {"message": "canned upstream unavailable", "type": "server_error"}}
        ).encode()
        self.send_response(503)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _ok(self, streaming):
        if not streaming:
            payload = json.dumps(
                {
                    "id": "chatcmpl-cooldown-ok",
                    "object": "chat.completion",
                    "created": int(time.time()),
                    "model": "canned-model",
                    "choices": [
                        {
                            "index": 0,
                            "finish_reason": "stop",
                            "message": {"role": "assistant", "content": FINAL},
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 1,
                        "completion_tokens": 1,
                        "total_tokens": 2,
                    },
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def chunk(delta, finish=None):
            frame = {
                "id": "chatcmpl-cooldown-ok",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": "canned-model",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
            self.wfile.flush()

        chunk({"role": "assistant", "content": ""})
        chunk({"content": FINAL})
        chunk({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    log(f"listening on 127.0.0.1:{PORT} mode={MODE}")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
