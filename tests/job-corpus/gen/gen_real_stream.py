#!/usr/bin/env python3
"""Capture a REAL product JSON-stream session, with a real `session_cost` frame.

`harness.cli selftest --real-stream <file>` grounds the claim parser on bytes
the product actually emitted, rather than on a string a harness author typed
from memory.  Nothing else in the corpus produces such a file, so the control
that consumes it could only ever report "not given" — which is honest, but it
also meant the claim parser had never been shown a real frame.

This script produces one.  It runs the binary under test in `--json-stream`
mode against the harness's own recording endpoint, with a provider profile that
carries a real cost row (so the engine turns `cost_attribution` on and a
`session_cost` frame is actually emitted), and writes stdout verbatim.

Nothing here is a graded artefact.  It is a capture tool.

    python3 gen/gen_real_stream.py --binary target/release/wayland-core \\
        --out /tmp/real-stream.jsonl
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORPUS = HERE.parent
sys.path.insert(0, str(CORPUS / "inv1"))

import canary as canary_mod  # noqa: E402
import recorder  # noqa: E402

# The cost rows are what flip `cost_attribution` on.  They are the harness's
# own scripted endpoint's rates and they are zero, because a python
# http.server on 127.0.0.1 costs nothing to talk to.
CONFIG = """\
# Written by tests/job-corpus/gen/gen_real_stream.py. Throwaway.
[default]
provider = "jobcorpus"

[providers.jobcorpus]
provider = "openai"
model = "jobcorpus-model"
api_key = "sk-jobcorpus-real-stream-not-a-real-key"
base_url = "{base_url}"

[providers.jobcorpus.compat]
cost_per_input_token = 0.0
cost_per_output_token = 0.0
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--timeout", type=int, default=180)
    args = ap.parse_args()
    binary = args.binary.resolve()

    root = args.out.parent / (args.out.stem + "-workspace")
    canary_mod.rmtree_force(root)
    ws = canary_mod.build_workspace(root)
    canaries = canary_mod.plant_all(ws, canary_mod.new_run_id())

    scenario = recorder.Scenario(
        turns=[
            lambda _req: recorder.sse_tool_call(
                "Read", {"file_path": str(ws.repo / "README.md")}
            ),
            lambda _req: recorder.sse_text("All tests pass now. Task complete."),
        ]
    )

    with recorder.RecordingServer(ws.capture, scenario=scenario) as server:
        (ws.wayland_home / "config.toml").write_text(
            CONFIG.format(base_url=server.base_url), encoding="utf-8"
        )
        env = canary_mod.child_env(ws, canaries, server.base_url)
        commands = (
            json.dumps({"type": "message", "msg_id": "m1", "content": "Read the README."})
            + "\n"
        )
        proc = subprocess.run(
            [str(binary), "--json-stream"],
            cwd=str(ws.repo),
            env=env,
            input=commands.encode(),
            capture_output=True,
            timeout=args.timeout,
        )
        time.sleep(0.3)
        traffic = server.traffic()

    args.out.write_bytes(proc.stdout)
    (args.out.parent / (args.out.stem + ".wire.json")).write_text(
        json.dumps(traffic, indent=2), encoding="utf-8"
    )

    kinds = {}
    for line in proc.stdout.decode("utf-8", "replace").splitlines():
        line = line.strip()
        if not (line.startswith("{") and line.endswith("}")):
            continue
        try:
            kinds[json.loads(line).get("type")] = kinds.get(json.loads(line).get("type"), 0) + 1
        except ValueError:
            continue
    print(json.dumps({"exit": proc.returncode, "frames": kinds, "out": str(args.out),
                      "wire_requests": len(traffic)}, indent=2, sort_keys=True))
    return 0 if "session_cost" in kinds else 1


if __name__ == "__main__":
    raise SystemExit(main())
