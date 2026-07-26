#!/usr/bin/env python3
"""Recording mock LLM endpoint for Phase 27 live intake measurement.

Speaks enough of the Anthropic `/v1/messages` streaming shape for the engine to
complete a turn, and writes every inbound request body verbatim to a JSONL file.
That capture is the instrument: it shows exactly what the engine put on the wire
for a given attachment, which is the only way to settle the degradation question
without reading source.

    python3 f27-mock-provider.py <port> <capture.jsonl>
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CAPTURE = None
REPLY = "F27-MOCK-REPLY"
# When set, the FIRST request is answered with a tool_use for this tool name
# (arguments read from F27_TOOL_INPUT), so a real tool path is exercised end to
# end. Every later request is answered with plain text.
TOOL_NAME = None
TOOL_INPUT = {}
_served = {"n": 0}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):  # silence the default stderr chatter
        pass

    def do_POST(self):  # noqa: N802 - BaseHTTPRequestHandler API
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw)
        except Exception:
            body = {"__unparsed__": raw.decode("utf-8", "replace")}
        with open(CAPTURE, "a", encoding="utf-8") as fh:
            fh.write(
                json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n"
            )

        _served["n"] += 1
        use_tool = TOOL_NAME is not None and _served["n"] == 1

        if use_tool:
            block_start = {
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_f27",
                    "name": TOOL_NAME,
                    "input": {},
                },
            }
            block_delta = {
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": json.dumps(TOOL_INPUT),
                },
            }
            stop_reason = "tool_use"
        else:
            block_start = {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            }
            block_delta = {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": REPLY},
            }
            stop_reason = "end_turn"

        events = [
            (
                "message_start",
                {
                    "type": "message_start",
                    "message": {
                        "id": "msg_f27",
                        "type": "message",
                        "role": "assistant",
                        "model": body.get("model", "mock"),
                        "content": [],
                        "stop_reason": None,
                        "usage": {"input_tokens": 1, "output_tokens": 1},
                    },
                },
            ),
            ("content_block_start", block_start),
            ("content_block_delta", block_delta),
            ("content_block_stop", {"type": "content_block_stop", "index": 0}),
            (
                "message_delta",
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": stop_reason, "stop_sequence": None},
                    "usage": {"output_tokens": 1},
                },
            ),
            ("message_stop", {"type": "message_stop"}),
        ]
        payload = "".join(
            f"event: {name}\ndata: {json.dumps(data)}\n\n" for name, data in events
        ).encode()

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        self.wfile.flush()


def main() -> int:
    global CAPTURE, TOOL_NAME, TOOL_INPUT
    port = int(sys.argv[1])
    CAPTURE = sys.argv[2]
    if len(sys.argv) > 4:
        TOOL_NAME = sys.argv[3]
        TOOL_INPUT = json.loads(sys.argv[4])
    open(CAPTURE, "w", encoding="utf-8").close()
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"F27-MOCK-READY port={port} capture={CAPTURE}", flush=True)
    srv.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
