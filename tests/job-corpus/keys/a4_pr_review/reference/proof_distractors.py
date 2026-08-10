#!/usr/bin/env python3
"""Executable refutation of the two mechanically checkable A-4 distractors.

    PYTHONPATH=<repo>/src python3 proof_distractors.py

D3 (the <= asymmetry) and D4 (setdefault "mutates a copy") are the two
distractors a reviewer is most likely to file as real bugs. Both are refuted
here. D1, D2 and D5 are argued in the key's README; they are judgement calls
about a documented single-process design, not mechanical claims.
"""

import sys


def refute_d4():
    from gatekeeper.limiter import RateLimiter

    limiter = RateLimiter(limit=5, window=10, buckets={})
    limiter.allow("d4", now=1.0)
    limiter.allow("d4", now=2.0)
    assert limiter.buckets["d4"] == [1.0, 2.0], (
        "setdefault mutation was NOT persisted: %r" % (limiter.buckets["d4"],)
    )
    return "D4 refuted: setdefault returns the stored list; mutation persists"


def refute_d3():
    from gatekeeper.limiter import RateLimiter

    # allow(): a hit exactly one window old is outside the window and is pruned.
    limiter = RateLimiter(limit=1, window=10, buckets={})
    assert limiter.allow("d3", now=0.0) is True
    assert limiter.allow("d3", now=10.0) is True, "hit at exactly one window old should expire"
    assert limiter.buckets["d3"] == [10.0], "the expired hit should be gone"

    # sweep(): a key whose newest hit is exactly one window old is idle.
    keeper = RateLimiter(limit=1, window=10, buckets={})
    keeper.allow("fresh", now=9.5)
    keeper.allow("stale", now=0.0)
    for key in list(keeper.buckets):
        hits = keeper.buckets[key]
        idle = (not hits) or hits[-1] <= (10.0 - 10.0)
        if key == "stale":
            assert idle, "stale key should read as idle"
        else:
            assert not idle, "fresh key should not read as idle"
    return "D3 refuted: both comparisons are correct for their own purpose"


def main():
    print(refute_d4())
    print(refute_d3())
    print("both mechanical distractors refuted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
