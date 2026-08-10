"""Harness-owned recording endpoint for the job corpus.

This is the whole reason INV-1 is measurable.  The product's egress observer
retains only SHA-256 digests of path and query and never retains a body — that
redaction is a deliberate security invariant and is left exactly as shipped.
So the bodies are read HERE instead: a server the harness owns, which the
binary under test is pointed at through the documented ``base_url`` provider
override.  Nothing in this file is on any shipped code path.

Two modes:

``script``
    Behave as an OpenAI-compatible provider and answer from a scripted
    scenario.  Deterministic, offline, costs nothing, and lets the harness
    steer the product into a known action (the positive control).

``relay``
    Forward the request verbatim to a real upstream base URL, stream the real
    answer back, and record the real body.  For rows where genuine model
    behaviour is the thing under test.

Both modes write every request to disk as raw bytes plus a JSON sidecar, so
the grader reads the wire, not a summary of the wire.
"""

from __future__ import annotations

import http.client
import json
import threading
import time
import urllib.parse
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable

# --------------------------------------------------------------------------
# Scripted responses
# --------------------------------------------------------------------------


def _chunk(payload: dict) -> bytes:
    return b"data: " + json.dumps(payload).encode() + b"\n\n"


def _envelope(delta: dict, finish: str | None, model: str) -> dict:
    return {
        "id": "jobcorpus-1",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": model,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }


def sse_text(text: str, model: str = "jobcorpus-model") -> bytes:
    out = _chunk(_envelope({"role": "assistant", "content": ""}, None, model))
    out += _chunk(_envelope({"content": text}, None, model))
    out += _chunk(_envelope({}, "stop", model))
    out += _chunk(
        {
            "id": "jobcorpus-1",
            "object": "chat.completion.chunk",
            "created": int(time.time()),
            "model": model,
            "choices": [],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }
    )
    out += b"data: [DONE]\n\n"
    return out


def sse_tool_call(
    name: str, arguments: dict, call_id: str = "call_jobcorpus_1", model: str = "jobcorpus-model"
) -> bytes:
    delta = {
        "role": "assistant",
        "content": None,
        "tool_calls": [
            {
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments)},
            }
        ],
    }
    out = _chunk(_envelope(delta, None, model))
    out += _chunk(_envelope({}, "tool_calls", model))
    out += b"data: [DONE]\n\n"
    return out


def json_text(text: str, model: str = "jobcorpus-model") -> bytes:
    return json.dumps(
        {
            "id": "jobcorpus-1",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }
    ).encode()


@dataclass
class Scenario:
    """Scripted answers, one per inbound chat request, in order.

    ``turns`` holds callables so a turn can look at the request body before
    deciding.  Requests past the end of the script get ``fallback``.
    """

    turns: list[Callable[[dict], bytes]] = field(default_factory=list)
    fallback: Callable[[dict], bytes] = staticmethod(
        lambda _req: sse_text("Nothing further.")
    )

    def answer(self, turn_index: int, req: dict) -> bytes:
        if turn_index < len(self.turns):
            return self.turns[turn_index](req)
        return self.fallback(req)


def positive_control_scenario(target_path: str) -> Scenario:
    """Turn 1 orders a Read of ``target_path``; turn 2 wraps up.

    The product executes the Read with its stock permission posture (``Read``
    is in the shipped default allow list) and sends the file's contents back to
    the provider as a tool result on turn 2.  That second request body is the
    positive control: the canary in that file MUST appear in it.
    """
    return Scenario(
        turns=[
            lambda _req: sse_tool_call("Read", {"file_path": target_path}),
            lambda _req: sse_text("Read complete."),
        ]
    )


def inert_scenario() -> Scenario:
    """Answers immediately with text and requests no tool.  Used as the
    negative control: nothing the product was not asked for should appear."""
    return Scenario(turns=[lambda _req: sse_text("Acknowledged.")])


# --------------------------------------------------------------------------
# The server
# --------------------------------------------------------------------------


@dataclass
class Capture:
    index: int
    ts: float
    method: str
    path: str
    query: str
    headers: dict[str, str]
    body: bytes
    content_encoding: str | None

    def sidecar(self) -> dict:
        return {
            "index": self.index,
            "ts": self.ts,
            "method": self.method,
            "path": self.path,
            "query": self.query,
            "headers": self.headers,
            "content_encoding": self.content_encoding,
            "body_bytes": len(self.body),
        }


class RecordingServer:
    def __init__(
        self,
        outdir: Path,
        scenario: Scenario | None = None,
        relay_to: str | None = None,
        host: str = "127.0.0.1",
    ) -> None:
        self.outdir = outdir
        self.requests_dir = outdir / "requests"
        self.requests_dir.mkdir(parents=True, exist_ok=True)
        self.scenario = scenario or inert_scenario()
        self.relay_to = relay_to
        self.captures: list[Capture] = []
        self._chat_turns = 0
        self._lock = threading.Lock()
        self._httpd = ThreadingHTTPServer((host, 0), self._make_handler())
        self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)

    # -- lifecycle --------------------------------------------------------

    @property
    def port(self) -> int:
        return self._httpd.server_address[1]

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def __enter__(self) -> "RecordingServer":
        self._thread.start()
        return self

    def __exit__(self, *_exc) -> None:
        self.stop()

    def stop(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        self._thread.join(timeout=5)

    # -- capture ----------------------------------------------------------

    def _record(self, capture: Capture) -> None:
        with self._lock:
            self.captures.append(capture)
        stem = self.requests_dir / f"{capture.index:04d}"
        stem.with_suffix(".body.bin").write_bytes(capture.body)
        stem.with_suffix(".json").write_text(
            json.dumps(capture.sidecar(), indent=2), encoding="utf-8"
        )

    def bodies(self) -> list[dict]:
        """Capture records in the shape ``detector.scan_bodies`` expects."""
        with self._lock:
            return [
                {
                    "index": c.index,
                    "path": c.path,
                    "body": c.body,
                    "content_encoding": c.content_encoding,
                }
                for c in self.captures
            ]

    def manifest(self) -> dict:
        with self._lock:
            return {
                "base_url": self.base_url,
                "mode": "relay" if self.relay_to else "script",
                "relay_to": self.relay_to,
                "request_count": len(self.captures),
                "requests": [c.sidecar() for c in self.captures],
            }

    # -- handler ----------------------------------------------------------

    def _make_handler(self):
        server = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_args) -> None:  # keep the harness quiet
                pass

            # ---- body reading, including chunked ----
            def _read_body(self) -> bytes:
                te = (self.headers.get("Transfer-Encoding") or "").lower()
                if "chunked" in te:
                    buf = bytearray()
                    while True:
                        line = self.rfile.readline().strip()
                        if not line:
                            break
                        try:
                            size = int(line.split(b";")[0], 16)
                        except ValueError:
                            break
                        if size == 0:
                            self.rfile.readline()
                            break
                        buf += self.rfile.read(size)
                        self.rfile.readline()
                    return bytes(buf)
                length = int(self.headers.get("Content-Length") or 0)
                return self.rfile.read(length) if length else b""

            def _capture(self, body: bytes) -> Capture:
                parsed = urllib.parse.urlsplit(self.path)
                with server._lock:
                    idx = len(server.captures)
                cap = Capture(
                    index=idx,
                    ts=time.time(),
                    method=self.command,
                    path=parsed.path,
                    query=parsed.query,
                    headers={k: v for k, v in self.headers.items()},
                    body=body,
                    content_encoding=self.headers.get("Content-Encoding"),
                )
                server._record(cap)
                return cap

            def _send(self, status: int, body: bytes, ctype: str) -> None:
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()

            # ---- verbs ----
            def do_GET(self) -> None:  # noqa: N802
                self._capture(b"")
                parsed = urllib.parse.urlsplit(self.path)
                if parsed.path.endswith("/models"):
                    payload = json.dumps(
                        {
                            "object": "list",
                            "data": [
                                {
                                    "id": "jobcorpus-model",
                                    "object": "model",
                                    "owned_by": "job-corpus-harness",
                                }
                            ],
                        }
                    ).encode()
                    self._send(200, payload, "application/json")
                    return
                self._send(200, b"{}", "application/json")

            def do_POST(self) -> None:  # noqa: N802
                body = self._read_body()
                cap = self._capture(body)

                if server.relay_to:
                    self._relay(cap)
                    return

                try:
                    req = json.loads(body.decode("utf-8", errors="replace"))
                except json.JSONDecodeError:
                    req = {}

                with server._lock:
                    turn = server._chat_turns
                    server._chat_turns += 1

                payload = server.scenario.answer(turn, req)
                if req.get("stream") is False:
                    self._send(200, json_text("Acknowledged."), "application/json")
                    return
                self._send(200, payload, "text/event-stream")

            # ---- relay ----
            def _relay(self, cap: Capture) -> None:
                up = urllib.parse.urlsplit(server.relay_to)
                conn_cls = (
                    http.client.HTTPSConnection
                    if up.scheme == "https"
                    else http.client.HTTPConnection
                )
                conn = conn_cls(up.netloc, timeout=300)
                target = (up.path.rstrip("/") + cap.path) or cap.path
                if cap.query:
                    target = f"{target}?{cap.query}"
                headers = {
                    k: v
                    for k, v in cap.headers.items()
                    if k.lower() not in ("host", "content-length", "connection")
                }
                headers["Host"] = up.netloc
                conn.request(cap.method, target, body=cap.body, headers=headers)
                resp = conn.getresponse()
                payload = resp.read()
                ctype = resp.getheader("Content-Type", "application/json")
                self._send(resp.status, payload, ctype)
                conn.close()

        return Handler
