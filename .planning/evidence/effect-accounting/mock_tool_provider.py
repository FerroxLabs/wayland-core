#!/usr/bin/env python3
"""Loopback OpenAI-compatible provider that asks for a DESTRUCTIVE tool call.

Claim B needs a turn that parks on a human approval. The cheapest way to get
one deterministically is a provider that always answers with a `Bash` tool
call whose command is unmistakably destructive, so the engine's approval
posture (`approvals = "prompt"`) must gate it.

Every request is logged with a `BILLED` line, same meter contract as
`mock_provider.py`, so "did the product reach a provider" is answered by this
server rather than by the product's own stdout.
"""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("MOCK_LOG", "/tmp/mock_tool_provider.log")
CMD = os.environ.get("MOCK_BASH_CMD", "rm -rf /tmp/effacc-destructive-target")
TOK_IN = int(os.environ.get("MOCK_TOKENS_IN", "10"))
TOK_OUT = int(os.environ.get("MOCK_TOKENS_OUT", "10"))


def log(msg):
    with open(LOG, "a") as f:
        f.write(f"{time.time():.3f} {msg}\n")
        f.flush()


def usage():
    return {"prompt_tokens": TOK_IN, "completion_tokens": TOK_OUT,
            "total_tokens": TOK_IN + TOK_OUT}


def tool_call():
    return {
        "id": "call_effacc_0001",
        "type": "function",
        "function": {"name": "Bash",
                     "arguments": json.dumps({"command": CMD})},
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            body = {}
        log(f"BILLED path={self.path} stream={body.get('stream')} "
            f"msgs={len(body.get('messages') or [])} tokens_in={TOK_IN}")
        if body.get("stream"):
            self._sse()
        else:
            self._json()

    def _json(self):
        payload = json.dumps({
            "id": "chatcmpl-effacc-tool", "object": "chat.completion",
            "created": int(time.time()), "model": "mock-model",
            "choices": [{"index": 0, "finish_reason": "tool_calls",
                         "message": {"role": "assistant", "content": None,
                                     "tool_calls": [tool_call()]}}],
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
            d = {"id": "chatcmpl-effacc-tool", "object": "chat.completion.chunk",
                 "created": int(time.time()), "model": "mock-model",
                 "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            if u is not None:
                d["usage"] = u
            self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
            self.wfile.flush()

        tc = tool_call()
        chunk({"role": "assistant", "content": None})
        chunk({"tool_calls": [{"index": 0, "id": tc["id"], "type": "function",
                               "function": {"name": "Bash", "arguments": ""}}]})
        chunk({"tool_calls": [{"index": 0,
                               "function": {"arguments": tc["function"]["arguments"]}}]})
        chunk({}, finish="tool_calls", u=usage())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8481
    log(f"listening on {port} cmd={CMD!r}")
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
