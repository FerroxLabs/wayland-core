"""A two-layer cache: a durable map, and a pending buffer in front of it.

The distinction matters more than it looks. `get` will happily answer out of the
pending buffer, so within a single pass nothing seems unusual. `get_committed`
will not: it only ever reads the durable layer. Anything that needs to be sure a
value has actually been made durable calls `get_committed`.

Which of the two layers a put lands in is decided by `CacheConfig.write_through`.
"""

from .config import CacheConfig


class ResolutionCache:
    def __init__(self, config=None):
        self.config = config or CacheConfig()
        self._durable = {}
        self._pending = {}
        self.stats = {"puts": 0, "hits": 0, "misses": 0, "flushes": 0, "evictions": 0}

    def put(self, key, value):
        self.stats["puts"] += 1
        if self.config.write_through:
            self._durable[key] = value
        else:
            self._pending[key] = value
        self._evict_if_needed()

    def get(self, key, default=None):
        """Read through both layers. Pending wins, because it is newer."""
        if key in self._pending:
            self.stats["hits"] += 1
            return self._pending[key]
        if key in self._durable:
            self.stats["hits"] += 1
            return self._durable[key]
        self.stats["misses"] += 1
        return default

    def get_committed(self, key, default=None):
        """Read only what is durable. The pending buffer is invisible here."""
        if key in self._durable:
            self.stats["hits"] += 1
            return self._durable[key]
        self.stats["misses"] += 1
        return default

    def flush(self):
        """Make everything pending durable."""
        self.stats["flushes"] += 1
        self._durable.update(self._pending)
        self._pending.clear()
        self._evict_if_needed()

    def discard(self, key):
        self._pending.pop(key, None)
        self._durable.pop(key, None)

    def _evict_if_needed(self):
        overflow = len(self._durable) - self.config.max_entries
        if overflow <= 0:
            return
        for key in list(self._durable)[:overflow]:
            del self._durable[key]
            self.stats["evictions"] += 1
