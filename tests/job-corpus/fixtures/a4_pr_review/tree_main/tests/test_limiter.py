import unittest

from gatekeeper import RateLimiter


class FixedWindow(unittest.TestCase):
    def test_allows_up_to_the_limit(self):
        limiter = RateLimiter(limit=2, window=10)
        self.assertTrue(limiter.allow("k1", now=0.0))
        self.assertTrue(limiter.allow("k1", now=1.0))

    def test_blocks_past_the_limit(self):
        limiter = RateLimiter(limit=2, window=10)
        limiter.allow("k2", now=0.0)
        limiter.allow("k2", now=1.0)
        self.assertFalse(limiter.allow("k2", now=2.0))

    def test_window_resets(self):
        limiter = RateLimiter(limit=1, window=10)
        self.assertTrue(limiter.allow("k3", now=0.0))
        self.assertTrue(limiter.allow("k3", now=11.0))


if __name__ == "__main__":
    unittest.main()
