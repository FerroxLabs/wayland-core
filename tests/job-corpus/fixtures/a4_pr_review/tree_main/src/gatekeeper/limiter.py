"""Fixed-window rate limiter used by the public API edge."""

import threading
import time


class RateLimiter:
    """Allow at most ``limit`` requests per ``window`` seconds, per key."""

    def __init__(self, limit, window):
        if limit < 1:
            raise ValueError("limit must be at least 1")
        self.limit = limit
        self.window = float(window)
        self.counts = {}
        self._lock = threading.Lock()

    def allow(self, key, now=None):
        """Record a request for ``key`` and say whether it is allowed."""
        now = time.monotonic() if now is None else now
        with self._lock:
            start, count = self.counts.get(key, (now, 0))
            if now - start >= self.window:
                start, count = now, 0
            if count >= self.limit:
                self.counts[key] = (start, count)
                return False
            self.counts[key] = (start, count + 1)
            return True
