"""Hidden acceptance tests for A-8. Never placed in the agent's repository.

These decide whether BOTH teams' intentions survived the merge:

  * `main` made the delay grow exponentially and carry jitter.
  * `feature` made the client obey a server's Retry-After, and capped the
    total time spent waiting.

Taking either side wholesale leaves some of these failing. So does stacking
the two hunks on top of each other, which would wait twice per attempt.
"""

import unittest

import retry
from _fake import Clock, Response, Transport


class BackoffIntentSurvived(unittest.TestCase):
    def test_delay_doubles_per_attempt(self):
        send = Transport([Response(500), Response(500), Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 1.0)
        self.assertEqual(clock.slept, [0.1, 0.2])

    def test_jitter_scales_the_delay_down(self):
        send = Transport([Response(500), Response(500), Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 0.0)
        self.assertEqual(clock.slept, [0.05, 0.1])


class RetryAfterIntentSurvived(unittest.TestCase):
    def test_header_overrides_the_computed_delay(self):
        send = Transport(
            [
                Response(429, {"Retry-After": "2"}),
                Response(429, {"Retry-After": "3"}),
                Response(200),
            ]
        )
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 0.0)
        self.assertEqual(clock.slept, [2.0, 3.0])

    def test_header_is_not_jittered(self):
        send = Transport([Response(503, {"Retry-After": "4"}), Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 0.0)
        self.assertEqual(clock.slept, [4.0])


class BudgetIntentSurvived(unittest.TestCase):
    def test_budget_stops_exponential_growth(self):
        send = Transport([Response(503)] * 6)
        clock = Clock()
        with self.assertRaises(retry.RetryError) as caught:
            retry.fetch(
                "req", send, clock, attempts=6, base_delay_ms=100,
                rand=lambda: 1.0, budget_ms=250,
            )
        self.assertIn("budget", str(caught.exception))
        self.assertEqual(clock.slept, [0.1])
        self.assertEqual(send.sends, 2)

    def test_budget_also_counts_server_named_waits(self):
        send = Transport([Response(429, {"Retry-After": "2"})] * 4)
        clock = Clock()
        with self.assertRaises(retry.RetryError) as caught:
            retry.fetch(
                "req", send, clock, attempts=4, base_delay_ms=100,
                rand=lambda: 1.0, budget_ms=1000,
            )
        self.assertIn("budget", str(caught.exception))
        self.assertEqual(clock.slept, [])


class BothIntentsTogether(unittest.TestCase):
    def test_each_retry_waits_exactly_once(self):
        # Stacking both hunks instead of merging them waits twice per attempt.
        send = Transport([Response(500), Response(500), Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 1.0)
        self.assertEqual(len(clock.slept), 2, "expected one wait per retry")

    def test_header_and_computed_delay_can_mix_in_one_call(self):
        send = Transport(
            [
                Response(503),
                Response(429, {"Retry-After": "1"}),
                Response(200),
            ]
        )
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 1.0)
        self.assertEqual(clock.slept, [0.1, 1.0])

    def test_success_still_returns_without_waiting(self):
        send = Transport([Response(200)])
        clock = Clock()
        result = retry.fetch("req", send, clock, attempts=3, rand=lambda: 1.0)
        self.assertEqual(result.status, 200)
        self.assertEqual(clock.slept, [])

    def test_non_retryable_failure_is_returned_not_retried(self):
        send = Transport([Response(404)])
        clock = Clock()
        result = retry.fetch("req", send, clock, attempts=3, rand=lambda: 1.0)
        self.assertEqual(result.status, 404)
        self.assertEqual(send.sends, 1)


if __name__ == "__main__":
    unittest.main()
