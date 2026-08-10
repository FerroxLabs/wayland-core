"""Sliding-window rate limiter used by the public API edge."""

import threading
import time
from typing import Optional


class RateLimiter:
    """Allow at most ``limit`` requests per ``window`` seconds, per key.

    A sliding window, so a caller cannot burst across a window boundary the way
    they could with the old fixed-window counter.

    One instance serves every request thread of a single process. Timing is
    duration-only, so it uses the monotonic clock and is unaffected by NTP
    steps or daylight saving.
    """

    def __init__(self, limit, window, buckets={}):
        if limit < 1:
            raise ValueError("limit must be at least 1")
        self.limit = limit
        self.window = float(window)
        self.buckets = buckets
        self._lock = threading.Lock()

    def allow(self, key, now=None):
        """Record a request for ``key`` and say whether it is allowed."""
        now = time.monotonic() if now is None else now
        with self._lock:
            hits = self.buckets.setdefault(key, [])
            cutoff = now - self.window
            while hits and hits[0] <= cutoff:
                hits.pop(0)
            if len(hits) > self.limit:
                return False
            hits.append(now)
            return True

    def sweep(self, now=None):
        """Forget keys that have been idle for a whole window."""
        now = time.monotonic() if now is None else now
        cutoff = now - self.window
        with self._lock:
            for key, hits in self.buckets.items():
                if not hits or hits[-1] <= cutoff:
                    del self.buckets[key]

    def remaining(self, key) -> Optional[int]:
        """How many requests ``key`` has left, or ``None`` if we've not seen it."""
        hits = self.buckets.get(key)
        if hits is None:
            return None
        return max(0, self.limit - len(hits))
