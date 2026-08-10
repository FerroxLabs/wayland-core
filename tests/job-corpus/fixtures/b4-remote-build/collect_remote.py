#!/usr/bin/env python3
"""Photograph the BUILD HOST after a B-4 run. Run this on the remote machine.

Emits, as JSON on stdout, the facts the grader needs and nothing the product
had a hand in producing: which processes for this run are still alive, which
partial files are still on disk, whether the build lock is still held, and the
build host's own ledger.

  python3 collect_remote.py --run-id <id> --workspace /path/to/checkout
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys


def home():
    return os.path.abspath(os.environ.get(
        "JOBCORPUS_B4_HOME", os.path.join(os.path.expanduser("~"), ".jobcorpus-b4")))


def processes(run_id):
    out = []
    if os.name == "nt":
        res = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "Get-CimInstance Win32_Process | Select-Object ProcessId,CommandLine | "
             "ConvertTo-Json -Depth 3"],
            capture_output=True, text=True, check=False)
        try:
            rows = json.loads(res.stdout or "[]")
        except json.JSONDecodeError:
            rows = []
        if isinstance(rows, dict):
            rows = [rows]
        for r in rows:
            cmd = r.get("CommandLine") or ""
            if run_id in cmd and "collect_remote" not in cmd:
                out.append({"pid": r.get("ProcessId"), "cmd": cmd})
    else:
        res = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True,
                             check=False)
        for line in res.stdout.splitlines():
            line = line.strip()
            if run_id in line and "collect_remote" not in line and " grep " not in line:
                pid, _, cmd = line.partition(" ")
                out.append({"pid": pid, "cmd": cmd.strip()})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--workspace", required=True)
    args = ap.parse_args()
    root, ws = home(), os.path.abspath(args.workspace)

    ledger = []
    lpath = os.path.join(root, "ledger.jsonl")
    if os.path.exists(lpath):
        with open(lpath, "r", encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    ledger.append(json.loads(line))
                except json.JSONDecodeError:
                    pass

    staging = os.path.join(ws, "dist", ".staging")
    partials = sorted(os.listdir(staging)) if os.path.isdir(staging) else []
    dist = os.path.join(ws, "dist")
    dist_files = sorted(f for f in os.listdir(dist)) if os.path.isdir(dist) else []

    lock = os.path.join(root, "build.lock")
    lock_present = os.path.exists(lock)
    lock_contents = ""
    if lock_present:
        with open(lock, "r", encoding="utf-8") as fh:
            lock_contents = fh.read().strip()

    seed_path = os.path.join(root, "remote-only-seed.txt")
    fp = None
    if os.path.exists(seed_path):
        with open(seed_path, "rb") as fh:
            fp = hashlib.sha256(fh.read().strip()).hexdigest()[:16]

    print(json.dumps({
        "host": platform.node(),
        "run_id": args.run_id,
        "seed_fingerprint": fp,
        "processes_alive": processes(args.run_id),
        "partial_files": partials,
        "dist_files": dist_files,
        "lock_present": lock_present,
        "lock_contents": lock_contents,
        "ledger": ledger,
    }, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
