#!/usr/bin/env python3
"""Executable demonstration that A-4's three material defects are real.

Run against a built A-4 fixture on branch pr/sliding-window:

    PYTHONPATH=<repo>/src python3 proof_of_bugs.py

Exits 0 only when all three defects reproduce. If this ever stops reproducing,
the A-4 key is grading opinions and must be rebuilt.
"""

import sys


def prove_m1():
    from gatekeeper.limiter import RateLimiter

    limiter = RateLimiter(limit=2, window=10, buckets={})
    allowed = sum(1 for i in range(5) if limiter.allow("m1", now=float(i) / 100))
    assert allowed == 3, "expected the off-by-one to admit 3 with limit=2, got %d" % allowed
    return "M1: limit=2 admitted %d requests in one window" % allowed


def prove_m2():
    from gatekeeper.limiter import RateLimiter

    a = RateLimiter(limit=1, window=10)
    b = RateLimiter(limit=1, window=10)
    a.allow("shared", now=0.0)
    assert b.buckets is a.buckets, "expected the default bucket dict to be shared"
    assert "shared" in b.buckets, "expected b to see a's traffic"
    return "M2: two independent limiters share one bucket dict"


def prove_m3():
    from gatekeeper.limiter import RateLimiter

    limiter = RateLimiter(limit=5, window=10, buckets={})
    limiter.allow("idle-a", now=0.0)
    limiter.allow("idle-b", now=0.0)
    try:
        limiter.sweep(now=100.0)
    except RuntimeError as exc:
        return "M3: sweep() raised RuntimeError: %s" % exc
    raise AssertionError("expected sweep() to raise RuntimeError, it did not")


def main():
    for prove in (prove_m1, prove_m2, prove_m3):
        print(prove())
    print("all three material defects reproduce")
    return 0


if __name__ == "__main__":
    sys.exit(main())
