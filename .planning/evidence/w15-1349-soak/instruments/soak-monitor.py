#!/usr/bin/env python3
"""w15/soak1349 status writer. Reads the driver's receipt.partial.json once a
minute and writes soak/<run>.status (latest) and soak/<run>.status.jsonl
(history). Never touches the driver or the product. Exits once the driver pid
is gone, after one final record."""
import datetime
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

RUN, PID = sys.argv[1], int(sys.argv[2])
W = Path("/root/w15-soak1349/soak")
ROOT = W / RUN


def driver_alive():
    try:
        return f"driver-{RUN}.py" in Path(f"/proc/{PID}/cmdline").read_bytes().decode(errors="replace")
    except OSError:
        return False


def builds():
    out = subprocess.run(["ps", "-eo", "comm", "--no-headers"], capture_output=True, text=True).stdout.split()
    return sum(1 for c in out if c in ("cargo", "rustc", "cargo-nextest", "mold", "ld.mold"))


def summarize(path):
    r = json.loads(path.read_text())
    cycles = r.get("cycles", [])
    rec = {"batches": len(r.get("batches", [])), "cycles": len(cycles),
           "passing": sum(1 for c in cycles if c.get("pass")), "elapsed_s": r.get("elapsed_seconds"),
           "completed": r.get("completed"), "error": (r.get("error") or "")[:300] or None}
    if r.get("batches"):
        b = r["batches"][-1]
        rec["last_batch"] = {"index": b["index"], "concurrency": b["concurrency"],
                             "quiescent_rss": b["quiescent_rss_bytes"], "peak_rss": b["rss_peak_bytes"]}
    if len(cycles) >= 200:
        first = statistics.median(c["quiescent_rss_bytes"] for c in cycles[:100])
        last = statistics.median(c["quiescent_rss_bytes"] for c in cycles[-100:])
        rec["running_rss"] = {"first100_median": first, "last100_median": last, "growth": last - first,
                              "formula_limit": max(first * .1, 32 * 1048576)}
    return rec


while True:
    alive = driver_alive()
    rec = {"utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
           "loadavg": Path("/proc/loadavg").read_text().strip(), "driver_alive": alive, "builds": builds()}
    final = ROOT / "receipt.json"
    source = final if (not alive and final.exists()) else ROOT / "receipt.partial.json"
    try:
        if source.exists():
            rec["from"] = source.name
            rec.update(summarize(source))
    except Exception as error:  # partial file is rewritten non-atomically each batch
        rec["read_error"] = repr(error)[:200]
    line = json.dumps(rec)
    with (W / f"{RUN}.status.jsonl").open("a") as history:
        history.write(line + "\n")
    tmp = W / f"{RUN}.status.tmp"
    tmp.write_text(line + "\n")
    os.replace(tmp, W / f"{RUN}.status")
    if not alive:
        break
    time.sleep(60)
