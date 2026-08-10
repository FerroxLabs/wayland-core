"""Summarise parsed access-log records."""

from collections import Counter


def status_classes(records):
    c = Counter()
    for r in records:
        c["%dxx" % (r["status"] // 100)] += 1
    return dict(c)


def top_paths(records, n=3):
    c = Counter(r["path"] for r in records)
    return [p for p, _ in sorted(c.items(), key=lambda kv: (-kv[1], kv[0]))[:n]]


def bytes_by_method(records):
    out = {}
    for r in records:
        out[r["method"]] = out.get(r["method"], 0) + r["bytes"]
    return out


def error_rate(records):
    if not records:
        return 0.0
    bad = sum(1 for r in records if r["status"] >= 500)
    return round(bad / len(records), 4)
