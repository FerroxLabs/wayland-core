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
import itertools
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


_CALL_SEQ = itertools.count(1)


def sse_tool_call(
    name: str, arguments: dict, call_id: str | None = None, model: str = "jobcorpus-model"
) -> bytes:
    """One scripted tool call.

    The id is unique per call by default.  Reusing one id across turns is not a
    cosmetic sin: the product's durable journal keys hook authority on
    (turn, call_id, index) and rejects the second turn outright — "duplicate
    hook phase authority ... PreToolUse" — so every tool call after the first
    silently does not run.  A scripted scenario that does that is measuring the
    harness's own bug, not the product.
    """
    if call_id is None:
        call_id = "call_jobcorpus_%d" % next(_CALL_SEQ)
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
    # A real OpenAI-compatible provider reports usage on a tool-call turn too.
    # Omitting it made the harness's own endpoint the reason a turn came out
    # "unpriced", which would have been the fixture incriminating the product.
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


def work_scenario(repo: Path, control_file: Path, readme: Path) -> Scenario:
    """Ordinary exploratory work: search the repo, read a file, look around.

    This is the arm that matters.  The ``posctl`` scenario asks for one file by
    name and the ``inert`` scenario asks for nothing at all in 0.19 seconds —
    neither of them is how a secret actually escapes.  A secret escapes while
    the agent is doing the job you gave it: grepping a tree, opening files,
    listing a directory, and shipping everything it found to the model.

    Turn 1 is the load-bearing one.  A single repository-wide search crosses
    BOTH a secret-shaped file (``.env``, whose canary must never appear) and an
    ordinary source file (``control_file``, whose canary must).  One grep, one
    request body, one detector: the control and the thing under test travel the
    identical route, so a clean result cannot be an artefact of the route being
    dead.
    """
    return Scenario(
        turns=[
            lambda _req: sse_tool_call(
                "Grep", {"pattern": "AWS_SECRET_ACCESS_KEY", "path": str(repo)}
            ),
            lambda _req: sse_tool_call("Read", {"file_path": str(control_file)}),
            lambda _req: sse_tool_call("Read", {"file_path": str(readme)}),
            lambda _req: sse_tool_call("Bash", {"command": "ls -la"}),
            lambda _req: sse_text("Explored the repository."),
        ]
    )


def inert_scenario() -> Scenario:
    """Answers immediately with text and requests no tool.  Used as the
    negative control: nothing the product was not asked for should appear."""
    return Scenario(turns=[lambda _req: sse_text("Acknowledged.")])


# --------------------------------------------------------------------------
# The server
# --------------------------------------------------------------------------


def _loads(raw: bytes):
    try:
        return json.loads(raw.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None


def _model_of(req: dict, response: bytes) -> str | None:
    """The model identity as it appears ON THE WIRE.

    Preference order is deliberate: what the product ASKED for is what the
    product is accountable for.  The provider's echo is the fallback, because a
    provider is free to substitute (and when it does, the substitution is the
    interesting fact, not the request).
    """
    asked = req.get("model")
    if isinstance(asked, str) and asked:
        return asked
    for obj in _response_objects(response):
        got = obj.get("model")
        if isinstance(got, str) and got:
            return got
    return None


def _response_objects(response: bytes) -> list[dict]:
    """Every JSON object in a response, plain or server-sent-events."""
    if not response:
        return []
    whole = _loads(response)
    if isinstance(whole, dict):
        return [whole]
    out: list[dict] = []
    for line in response.split(b"\n"):
        line = line.strip()
        if line.startswith(b"data:"):
            line = line[5:].strip()
        if not line or line == b"[DONE]":
            continue
        obj = _loads(line)
        if isinstance(obj, dict):
            out.append(obj)
    return out


#: Token-count field names, in the two shapes the wire actually uses.
_USAGE_KEYS = (
    ("prompt_tokens", "completion_tokens"),  # OpenAI-compatible
    ("input_tokens", "output_tokens"),  # Anthropic-compatible
)


def _usage_from_response(response: bytes) -> dict:
    """Token counts as the PROVIDER reported them, or nothing.

    Returning nothing is a first-class outcome: a request whose token cost the
    harness could not read is a request the harness cannot price, and saying so
    is what stops `$0.00` being manufactured out of silence.
    """
    for obj in _response_objects(response):
        usage = obj.get("usage")
        if not isinstance(usage, dict):
            continue
        for in_key, out_key in _USAGE_KEYS:
            if in_key in usage or out_key in usage:
                return {
                    "input_tokens": _int_or_none(usage.get(in_key)),
                    "output_tokens": _int_or_none(usage.get(out_key)),
                    "source": "provider_usage",
                }
    return {"input_tokens": None, "output_tokens": None, "source": None}


def _int_or_none(value):
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


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
    #: The bytes this endpoint sent back.  Retained because the provider's own
    #: `usage` block is the only honest token count available to the harness:
    #: a token figure the PRODUCT reports is a claim, and INV-5 exists to check
    #: claims against something else.
    response_body: bytes = b""
    response_status: int | None = None

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
            "response_status": self.response_status,
            "response_bytes": len(self.response_body),
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
        self._flush(capture)

    def _flush(self, capture: Capture) -> None:
        stem = self.requests_dir / f"{capture.index:04d}"
        stem.with_suffix(".body.bin").write_bytes(capture.body)
        if capture.response_body:
            stem.with_suffix(".response.bin").write_bytes(capture.response_body)
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

    # -- metering ---------------------------------------------------------

    def traffic(self) -> list[dict]:
        """One record per model-completion request the harness actually served.

        Everything in it is read off the wire: the model identity out of the
        request body (or the provider's own echo of it), and the token counts
        out of the provider's `usage` block.  Nothing here is anything the
        product said about itself, which is the entire point — INV-5 has to
        reconcile the product's account against an account it did not write.
        """
        with self._lock:
            caps = list(self.captures)
        out: list[dict] = []
        for cap in caps:
            if cap.method != "POST" or not cap.body:
                continue
            req = _loads(cap.body)
            if not isinstance(req, dict) or "messages" not in req and "prompt" not in req:
                # Not a completion call (embeddings, token counting, health).
                # Still recorded, but as its own kind so it cannot inflate the
                # turn count the product is reconciled against.
                out.append(
                    {
                        "kind": "provider_other",
                        "index": cap.index,
                        "ts": cap.ts,
                        "path": cap.path,
                        "request_bytes": len(cap.body),
                    }
                )
                continue
            usage = _usage_from_response(cap.response_body)
            out.append(
                {
                    "kind": "provider_request",
                    "index": cap.index,
                    "ts": cap.ts,
                    "path": cap.path,
                    "host": self.relay_to or self.base_url,
                    "model": _model_of(req, cap.response_body),
                    "stream": bool(req.get("stream")),
                    "request_bytes": len(cap.body),
                    "response_bytes": len(cap.response_body),
                    "input_tokens": usage.get("input_tokens"),
                    "output_tokens": usage.get("output_tokens"),
                    "usage_source": usage.get("source"),
                }
            )
        return out

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

            def _send(
                self, status: int, body: bytes, ctype: str, cap: Capture | None = None
            ) -> None:
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()
                if cap is not None:
                    cap.response_status = status
                    cap.response_body = body
                    server._flush(cap)

            # ---- verbs ----
            def do_GET(self) -> None:  # noqa: N802
                cap = self._capture(b"")
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
                    self._send(200, payload, "application/json", cap)
                    return
                self._send(200, b"{}", "application/json", cap)

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
                    self._send(200, json_text("Acknowledged."), "application/json", cap)
                    return
                self._send(200, payload, "text/event-stream", cap)

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
                self._send(resp.status, payload, ctype, cap)
                conn.close()

        return Handler
