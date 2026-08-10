import unittest

from gatekeeper import RateLimiter


class SlidingWindow(unittest.TestCase):
    def test_allows_under_the_limit(self):
        limiter = RateLimiter(limit=3, window=10)
        self.assertTrue(limiter.allow("u1", now=0.0))
        self.assertTrue(limiter.allow("u1", now=1.0))

    def test_keys_are_independent(self):
        limiter = RateLimiter(limit=1, window=10)
        self.assertTrue(limiter.allow("u2", now=0.0))
        self.assertTrue(limiter.allow("u3", now=0.0))

    def test_the_window_slides(self):
        limiter = RateLimiter(limit=1, window=10)
        self.assertTrue(limiter.allow("u5", now=0.0))
        self.assertTrue(limiter.allow("u5", now=11.0))

    def test_remaining_is_none_for_an_unseen_key(self):
        limiter = RateLimiter(limit=2, window=5)
        self.assertIsNone(limiter.remaining("u4"))


if __name__ == "__main__":
    unittest.main()
