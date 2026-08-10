import unittest

import retry
from _fake import Clock, Response, Transport


class RetryAfterHeader(unittest.TestCase):
    def test_server_named_delay_is_obeyed(self):
        send = Transport([Response(429, {"Retry-After": "2"}), Response(200)])
        clock = Clock()
        result = retry.fetch("req", send, clock, attempts=3, base_delay_ms=100)
        self.assertEqual(result.status, 200)
        self.assertEqual(clock.slept, [2.0])


class RetryBudget(unittest.TestCase):
    def test_budget_stops_the_retries_early(self):
        send = Transport([Response(503)] * 5)
        clock = Clock()
        with self.assertRaises(retry.RetryError) as caught:
            retry.fetch("req", send, clock, attempts=5, base_delay_ms=100, budget_ms=150)
        self.assertIn("budget", str(caught.exception))
        self.assertLess(send.sends, 5)

    def test_no_budget_means_no_cap(self):
        send = Transport([Response(503), Response(200)])
        clock = Clock()
        result = retry.fetch("req", send, clock, attempts=3, base_delay_ms=100)
        self.assertEqual(result.status, 200)


if __name__ == "__main__":
    unittest.main()
