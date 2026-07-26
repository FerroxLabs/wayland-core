#!/usr/bin/env python3
"""Render a captured outbound-request JSONL into a readable evidence file.

The capture is the raw body the engine PUT ON THE WIRE. This renders only the
parts that bear on Phase 27's questions — whether an image part survived, and
what a tool actually returned — so the degradation pair can be compared by eye
as well as by digest.

    f27-extract-wire.py <capture.jsonl> <out.txt>
"""

import json
import sys


def main() -> int:
    src, dst = sys.argv[1], sys.argv[2]
    lines = []
    try:
        raw = open(src, encoding="utf-8").read().splitlines()
    except OSError as exc:
        open(dst, "w", encoding="utf-8").write(f"NO CAPTURE: {exc}\n")
        return 0
    for line in raw:
        if not line.strip():
            continue
        body = json.loads(line)["body"]
        for msg in body.get("messages", []):
            content = msg.get("content")
            if not isinstance(content, list):
                continue
            for part in content:
                kind = part.get("type")
                if kind == "image":
                    src_obj = part["source"]
                    lines.append(
                        "IMAGE part media_type=%s data_len=%d"
                        % (src_obj["media_type"], len(src_obj["data"]))
                    )
                elif kind == "text":
                    lines.append("TEXT %r" % str(part.get("text"))[:200])
                elif kind == "tool_result":
                    lines.append(
                        "TOOL_RESULT is_error=%s %r"
                        % (part.get("is_error"), str(part.get("content"))[:400])
                    )
    if not lines:
        lines = ["NO OUTBOUND REQUEST (refused before any provider call was made)"]
    open(dst, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
