#!/usr/bin/env python3
"""Canned OpenAI-compatible endpoint for the 22-remaining mid-flight-monitor proof.

Serves POST /v1/chat/completions (SSE and non-streaming). For the first
$CANNED_TOOL_TURNS requests it returns a `Read` tool call whose path differs in
one VOLATILE directory component only, so:

  * the raw invocation differs every turn  -> LoopGuard's exact-repetition owner
    does not fire, and
  * MidFlightMonitor::root_cause_signature collapses them to one signature
    -> the repeated-error path is what is under test.

After that it returns plain text so the turn can terminate.

No credential of any kind is authenticated. The only key-shaped value involved
is a synthetic literal supplied by the caller. Every request is appended to
$CANNED_LOG so the caller can prove the product actually reached THIS endpoint
rather than the ANTHROPIC_API_KEY that /root/.wayland/.env injects regardless
of what the shell unsets (LANE-BRIEF section 3b-ii).
"""

import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("CANNED_LOG", "/tmp/canned_provider.log")
PORT = int(os.environ.get("CANNED_PORT", "18733"))
TOOL_TURNS = int(os.environ.get("CANNED_TOOL_TURNS", "6"))
FINAL = os.environ.get("CANNED_FINAL", "CANNED_TURN_DONE")

STATE = {"n": 0}


def log(msg):
    with open(LOG, "a") as fh:
        fh.write(f"{time.time():.3f} {msg}\n")
        fh.flush()


def tool_arguments(n):
    # Volatile middle directory; stable scope (/tmp) and stable basename.
    return json.dumps({"file_path": f"/tmp/wl22r-run-{n}/wl22r-missing.txt"})


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
        turn = STATE["n"]
        log(
            f"POST {self.path} turn={turn} stream={body.get('stream')} "
            f"model={body.get('model')} msgs={len(body.get('messages') or [])} "
            f"tools={len(body.get('tools') or [])}"
        )
        emit_tool = turn <= TOOL_TURNS
        if body.get("stream"):
            self._sse(turn, emit_tool)
        else:
            self._json(turn, emit_tool)

    def _envelope(self, message, finish):
        return {
            "id": f"chatcmpl-canned-{STATE['n']}",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": "canned-model",
            "choices": [{"index": 0, "finish_reason": finish, "message": message}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }

    def _json(self, turn, emit_tool):
        if emit_tool:
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": f"call_wl22r_{turn}",
                        "type": "function",
                        "function": {"name": "Read", "arguments": tool_arguments(turn)},
                    }
                ],
            }
            finish = "tool_calls"
        else:
            message = {"role": "assistant", "content": FINAL}
            finish = "stop"
        payload = json.dumps(self._envelope(message, finish)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _sse(self, turn, emit_tool):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def chunk(delta, finish=None):
            frame = {
                "id": f"chatcmpl-canned-{turn}",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": "canned-model",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
            self.wfile.flush()

        chunk({"role": "assistant", "content": ""})
        if emit_tool:
            chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": f"call_wl22r_{turn}",
                            "type": "function",
                            "function": {
                                "name": "Read",
                                "arguments": tool_arguments(turn),
                            },
                        }
                    ]
                }
            )
            chunk({}, "tool_calls")
        else:
            chunk({"content": FINAL})
            chunk({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    log(f"listening on 127.0.0.1:{PORT} tool_turns={TOOL_TURNS}")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
