#!/usr/bin/env python3
"""Canned OpenAI-compatible endpoint for the 22-remaining learned-policy proof.

Scripts one parent turn and one delegated child turn so the SAME on-disk
permissions policy can be observed applying to the child and NOT to the parent.
The only variable between the two `Read` calls is the caller class.

Discriminator: the FIRST user message. For the parent it is the operator's
stdin; for a delegated child it is the goal THIS server wrote, which carries
CHILD_MARKER. (The tool list is not usable: the session offers only 8 core
tools plus `ToolSearch` and discovers the rest lazily, so `Delegate` is absent
from the parent's advertised list as well. The engine dispatches by registry
lookup, so a canned tool call for an unadvertised tool still runs.)

Script:
  parent, call 1  -> Read  /tmp/wl22p/parent-probe.txt   (Root: must NOT be
                                                          denied by the policy)
  parent, call 2  -> Delegate { goal: ... }
  parent, call 3+ -> final text
  child,  call 1  -> Read  /tmp/wl22p/child-probe.txt    (SubAgent: MUST be
                                                          denied by the policy)
  child,  call 2+ -> final text

No credential of any kind is authenticated.
"""

import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("CANNED_LOG", "/tmp/canned_delegate.log")
PORT = int(os.environ.get("CANNED_PORT", "18734"))
PARENT_PATH = os.environ.get("PARENT_PATH", "/tmp/wl22p/parent-probe.txt")
CHILD_PATH = os.environ.get("CHILD_PATH", "/tmp/wl22p/child-probe.txt")
# Set to "0" to run the one-variable control in which the parent never
# delegates, so no sub-agent dispatch happens at all.
DELEGATE = os.environ.get("CANNED_DELEGATE", "1") == "1"
CHILD_MARKER = "WL22P-CHILD-MARKER"

STATE = {"parent": 0, "child": 0, "probe": 0}


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
        tools = body.get("tools") or []
        # Discriminator. The tool list is NOT usable for this: the session
        # exposes only 8 core tools plus `ToolSearch` and discovers the rest
        # lazily, so `Delegate` is absent from the parent's list too. What IS
        # reliably different is the FIRST user message: for the parent it is
        # the operator's stdin, for a delegated child it is the goal this
        # server wrote, which carries CHILD_MARKER.
        first_user = next(
            (
                m.get("content")
                for m in (body.get("messages") or [])
                if isinstance(m, dict) and m.get("role") == "user"
            ),
            "",
        )
        # The liveness probe must NOT advance the script, or every scripted
        # step shifts by one and the parent's first real turn gets the second
        # action. (It did, on the first run of this harness.)
        if body.get("model") == "probe":
            log(f"POST {self.path} role=probe (script not advanced)")
            self._json("probe", 0, ("text", None, None))
            return
        is_child = CHILD_MARKER in json.dumps(first_user)
        role = "child" if is_child else "parent"
        STATE[role] += 1
        step = STATE[role]
        # The delegated child has its OWN output sink, so its tool results never
        # appear in the parent's JSON stream. They DO appear here: the engine
        # feeds each tool result back into the child's next request as a `tool`
        # message. That is the product's own conversation state, which is the
        # only place a caller can observe what the child's dispatch produced.
        last_tool = next(
            (
                m.get("content")
                for m in reversed(body.get("messages") or [])
                if isinstance(m, dict) and m.get("role") == "tool"
            ),
            None,
        )
        log(
            f"POST {self.path} role={role} step={step} stream={body.get('stream')} "
            f"tools={len(tools)}"
        )
        if last_tool is not None:
            log(f"    last_tool_result[{role}:{step}] = {json.dumps(last_tool)[:400]}")
        action = self._script(role, step)
        if body.get("stream"):
            self._sse(role, step, action)
        else:
            self._json(role, step, action)

    def _script(self, role, step):
        if role == "parent":
            if step == 1:
                return ("tool", "Read", {"file_path": PARENT_PATH})
            if step == 2 and DELEGATE:
                return (
                    "tool",
                    "Delegate",
                    {
                        "goal": (
                            f"{CHILD_MARKER} Read the file {CHILD_PATH} "
                            "and report its first line."
                        ),
                        "context": "22-remaining learned-policy proof",
                        "max_iterations": 3,
                    },
                )
            return ("text", None, None)
        if step == 1:
            return ("tool", "Read", {"file_path": CHILD_PATH})
        return ("text", None, None)

    def _message(self, role, step, action):
        kind, name, args = action
        if kind == "tool":
            return (
                {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": f"call_{role}_{step}",
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": json.dumps(args),
                            },
                        }
                    ],
                },
                "tool_calls",
            )
        return ({"role": "assistant", "content": f"WL22P_{role.upper()}_DONE"}, "stop")

    def _json(self, role, step, action):
        message, finish = self._message(role, step, action)
        payload = json.dumps(
            {
                "id": f"chatcmpl-{role}-{step}",
                "object": "chat.completion",
                "created": int(time.time()),
                "model": "canned-model",
                "choices": [
                    {"index": 0, "finish_reason": finish, "message": message}
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

    def _sse(self, role, step, action):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def chunk(delta, finish=None):
            frame = {
                "id": f"chatcmpl-{role}-{step}",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": "canned-model",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
            self.wfile.flush()

        message, finish = self._message(role, step, action)
        chunk({"role": "assistant", "content": ""})
        if "tool_calls" in message:
            call = message["tool_calls"][0]
            chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": call["id"],
                            "type": "function",
                            "function": call["function"],
                        }
                    ]
                }
            )
        else:
            chunk({"content": message["content"]})
        chunk({}, finish)
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    log(f"listening on 127.0.0.1:{PORT} delegate={DELEGATE}")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
