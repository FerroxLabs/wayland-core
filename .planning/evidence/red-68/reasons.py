#!/usr/bin/env python3
import re, sys, collections

LINE = re.compile(r"""(?:^|\s)(?:TRY\s+\d+\s+)?(?P<status>[A-Z][A-Z0-9]*(?:[+-][A-Z0-9]+)*)\s+\[\s*[\d.]+s\]\s*\(\s*\d+\s*/\s*\d+\s*\)\s+(?P<test>\S.*?)\s*$""", re.VERBOSE)
NON_FAILURE = {"PASS","SKIP","SLOW","LEAK","PASS+LK","START","TRY"}
TS = re.compile(r"^\S*Z\s")

path = sys.argv[1]
lo = int(sys.argv[2]) if len(sys.argv) > 2 else 1
hi = int(sys.argv[3]) if len(sys.argv) > 3 else 10**9

lines = []
with open(path, errors="replace") as fh:
    for i, raw in enumerate(fh, 1):
        if lo <= i <= hi:
            lines.append(TS.sub("", raw.rstrip("\n")))

cur = None
out = collections.OrderedDict()
for ln in lines:
    m = LINE.search(ln)
    if m:
        st = m.group("status")
        if st not in NON_FAILURE:
            cur = m.group("test")
            out.setdefault(cur, [])
        else:
            cur = None
        continue
    if cur is not None:
        s = ln.strip()
        if s:
            out[cur].append(s)

for test, body in out.items():
    reason = []
    for i, s in enumerate(body):
        if "panicked at" in s:
            reason = body[i:i+4]
            break
    if not reason:
        reason = [s for s in body if s and not s.startswith(("running","test ","failures:","test result:","note: run","stdout","stderr"))][:4]
    print(f"### {test}")
    for r in reason:
        print(f"    {r}")
    print()
