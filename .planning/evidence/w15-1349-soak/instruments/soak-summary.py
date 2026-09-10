#!/usr/bin/env python3
"""w15/soak1349: summarize a finished mixed-soak receipt.json. Arithmetic over
the receipt only. Usage: soak-summary.py <receipt.json> <out-summary.txt> <out-batches.csv>"""
import collections
import hashlib
import json
import statistics
import sys
from pathlib import Path

MIXED_LIMIT = 127151104
CONFIRMATION_LIMIT = 33554432
src, out, csv_out = map(Path, sys.argv[1:4])
raw = src.read_bytes()
r = json.loads(raw)
cycles, batches = r.get("cycles", []), r.get("batches", [])
L = []
p = L.append
p("receipt        %s" % src)
p("receipt sha256 %s" % hashlib.sha256(raw).hexdigest())
for k in ("source", "build_info", "build_profile", "binary_sha256", "driver_sha256", "platform", "cpu_count",
          "network_namespace", "routes", "soak_started", "finished", "elapsed_seconds", "completed", "error",
          "rss_bound_pass", "rss_windows", "cleanup", "loadavg_start", "loadavg_end", "core_pids",
          "owned_process_resets", "rss_unavailable"):
    v = r.get(k)
    if isinstance(v, str):
        v = v.strip()
    if k == "rss_unavailable" and v:
        v = "%d entries" % len(v)
    p("%-15s %s" % (k, v))
p("MemTotal        %s" % next((l for l in r.get("meminfo", "").splitlines() if l.startswith("MemTotal")), ""))
p("")
p("batches %d   cycles %d   passing %d   failing %d" % (len(batches), len(cycles),
  sum(1 for c in cycles if c.get("pass")), sum(1 for c in cycles if not c.get("pass"))))
by = collections.defaultdict(lambda: [0, 0])
for b in batches:
    for c in b["rows"]:
        key = (b["concurrency"], c["reader"], c["text_bytes"])
        by[key][1] += 1
        by[key][0] += bool(c.get("pass"))
p("")
p("per-row pass count (concurrency, reader, payload bytes): pass/total")
for key in sorted(by):
    p("  conc %2d  %-12s %9d   %d/%d" % (key + tuple(by[key])))
conc = collections.Counter(b["concurrency"] for b in batches)
p("batches by concurrency: %s" % dict(sorted(conc.items())))
p("")
w = r.get("rss_windows")
if w:
    g = w["growth_bytes"]
    p("RSS VERDICT (driver's own formula max(10% of first-100 median, 32 MiB))")
    p("  first-100 quiescent median  %d" % w["first_100_median_bytes"])
    p("  last-100 quiescent median   %d" % w["last_100_median_bytes"])
    p("  growth                      %d" % g)
    p("  driver max_growth_bytes     %d" % w["max_growth_bytes"])
    p("  rss_bound_pass              %s" % r.get("rss_bound_pass"))
    p("  growth <= 127,151,104 (original mixed limit)        %s  (%.3fx)" % (g <= MIXED_LIMIT, g / MIXED_LIMIT))
    p("  growth <= 33,554,432 (original confirmation limit)  %s  (%.3fx)" % (g <= CONFIRMATION_LIMIT, g / CONFIRMATION_LIMIT))
else:
    p("RSS VERDICT: rss_windows absent -- the driver never reached its RSS computation")
if len(cycles) >= 2:
    ys = [c["quiescent_rss_bytes"] for c in cycles]
    xs = list(range(len(ys)))
    mx, my = statistics.fmean(xs), statistics.fmean(ys)
    sxx = sum((x - mx) ** 2 for x in xs)
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    slope = sxy / sxx
    ss_res = sum((y - (my + slope * (x - mx))) ** 2 for x, y in zip(xs, ys))
    ss_tot = sum((y - my) ** 2 for y in ys) or 1
    p("  informational: LSQ slope quiescent RSS vs cycle index %.1f B/cycle, R^2 %.4f" % (slope, 1 - ss_res / ss_tot))
    half = len(ys) // 2
    if half >= 2:
        xs2, ys2 = xs[half:], ys[half:]
        mx2, my2 = statistics.fmean(xs2), statistics.fmean(ys2)
        s2 = sum((x - mx2) * (y - my2) for x, y in zip(xs2, ys2)) / sum((x - mx2) ** 2 for x in xs2)
        p("  informational: second-half slope %.1f B/cycle" % s2)
    fh = [c["headers_ms"] for c in cycles[:100] if c.get("headers_ms") is not None]
    lh = [c["headers_ms"] for c in cycles[-100:] if c.get("headers_ms") is not None]
    if fh and lh:
        p("  informational: mean headers_ms first-100 %.2f, last-100 %.2f" % (statistics.fmean(fh), statistics.fmean(lh)))
    peak = max(b["rss_peak_bytes"] for b in batches)
    p("  informational: max batch peak RSS %d" % peak)
fails = [(b["index"], b["concurrency"], c) for b in batches for c in b["rows"] if not c.get("pass")]
if fails:
    p("")
    p("FAILING ROWS (first 10)")
    for idx, cc, c in fails[:10]:
        keep = {k: c.get(k) for k in ("reader", "text_bytes", "received_text_bytes", "terminal_count", "errors",
                                       "turn_ms", "prompt_status", "delete_status", "delete_ms",
                                       "get_after_delete_status", "fixture_active_before_delete", "read_error")}
        p("  batch %d conc %d %s" % (idx, cc, json.dumps(keep)[:900]))
out.write_text("\n".join(L) + "\n")
with csv_out.open("w") as f:
    f.write("index,concurrency,elapsed_seconds,rows,passing,quiescent_rss_bytes,rss_peak_bytes\n")
    for b in batches:
        f.write("%d,%d,%.3f,%d,%d,%d,%d\n" % (b["index"], b["concurrency"], b["elapsed_seconds"], len(b["rows"]),
                sum(1 for c in b["rows"] if c.get("pass")), b["quiescent_rss_bytes"], b["rss_peak_bytes"]))
print("\n".join(L))
