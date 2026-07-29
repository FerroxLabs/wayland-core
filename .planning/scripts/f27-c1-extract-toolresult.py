#!/usr/bin/env python3
"""Extract the tool_result the engine put back on the wire for a captured turn.

The capture is what the mock provider recorded, so this reads the ENGINE's own
outbound request bodies — the tool_result block is the engine telling the model
what the tool returned. That is the measurement: it is what a real provider
would have received, not what an internal function returned.

    f27-c1-extract-toolresult.py <capture.jsonl>
"""

import json
import sys


def blocks(msg):
    content = msg.get("content")
    if isinstance(content, list):
        return content
    return []


def main():
    path = sys.argv[1]
    try:
        lines = [json.loads(x) for x in open(path, encoding="utf-8") if x.strip()]
    except FileNotFoundError:
        print("  NO CAPTURE — the engine never reached the mock provider")
        return

    if not lines:
        print("  EMPTY CAPTURE — the engine never reached the mock provider")
        return

    print(f"  captured requests: {len(lines)}")
    found = False
    for entry in lines:
        for msg in entry.get("body", {}).get("messages", []):
            for b in blocks(msg):
                if b.get("type") == "tool_result":
                    found = True
                    c = b.get("content")
                    if isinstance(c, list):
                        c = " ".join(
                            x.get("text", "") for x in c if isinstance(x, dict)
                        )
                    text = str(c).replace("\n", " ")
                    err = b.get("is_error", False)
                    print(f"  TOOL_RESULT is_error={err}: {text[:400]}")
    if not found:
        print("  NO tool_result ON THE WIRE — the tool never returned to the model")


if __name__ == "__main__":
    main()
