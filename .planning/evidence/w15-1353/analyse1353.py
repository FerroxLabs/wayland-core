#!/usr/bin/env python3
"""wayland#1353 c4: summarise the interleaved A (b37b69505) / B (a8fd1ac9c) samples.

Every sample is used; none is dropped. Rounds alternate A,B / B,A, so the paired
difference B-A within a round cancels slow drift in host load.
"""
import csv
import random
import statistics
import sys

path = sys.argv[1]
rows = list(csv.DictReader(open(path)))
bad = [r for r in rows if r["exit"] != "0" or "1 passed; 0 failed" not in r["result"]]
arms = {"A": [], "B": []}
by_round = {}
for r in rows:
    s = float(r["seconds"])
    arms[r["arm"]].append(s)
    by_round.setdefault(int(r["round"]), {})[r["arm"]] = s

print(f"samples={len(rows)} failed_or_unexpected={len(bad)}")
for arm, xs in arms.items():
    print(f"arm={arm} n={len(xs)} median={statistics.median(xs):.3f} mean={statistics.mean(xs):.3f} "
          f"stdev={statistics.stdev(xs):.3f} min={min(xs):.3f} max={max(xs):.3f}")

pairs = [(k, v["B"] - v["A"]) for k, v in sorted(by_round.items()) if "A" in v and "B" in v]
diffs = [d for _, d in pairs]
print("paired B-A per round:", " ".join(f"r{k}:{d:+.3f}" for k, d in pairs))
print(f"paired n={len(diffs)} median_diff={statistics.median(diffs):+.3f}s mean_diff={statistics.mean(diffs):+.3f}s "
      f"B_slower_rounds={sum(d > 0 for d in diffs)}/{len(diffs)}")

rng = random.Random(1353)
boot_med, boot_ratio = [], []
for _ in range(20000):
    sample = [rng.choice(diffs) for _ in diffs]
    boot_med.append(statistics.median(sample))
    a = [rng.choice(arms["A"]) for _ in arms["A"]]
    b = [rng.choice(arms["B"]) for _ in arms["B"]]
    boot_ratio.append(statistics.median(b) / statistics.median(a))
boot_med.sort(); boot_ratio.sort()
lo, hi = int(0.025 * len(boot_med)), int(0.975 * len(boot_med)) - 1
print(f"bootstrap95 median paired diff: [{boot_med[lo]:+.3f}, {boot_med[hi]:+.3f}] s (seed 1353, 20000 resamples)")
print(f"median ratio B/A = {statistics.median(arms['B']) / statistics.median(arms['A']):.4f} "
      f"bootstrap95 [{boot_ratio[lo]:.4f}, {boot_ratio[hi]:.4f}]")

loads = [float(r["load1"]) for r in rows]
print(f"load1 at sample start: min={min(loads):.2f} median={statistics.median(loads):.2f} max={max(loads):.2f} (96 CPU)")
for arm in "AB":
    la = [float(r["load1"]) for r in rows if r["arm"] == arm]
    print(f"load1 arm={arm} median={statistics.median(la):.2f}")
if bad:
    print("UNEXPECTED:", bad)
