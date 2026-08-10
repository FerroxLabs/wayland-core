import unittest

import retry
from _fake import Clock, Response, Transport


class Backoff(unittest.TestCase):
    def test_delay_doubles_between_attempts(self):
        send = Transport([Response(500), Response(500), Response(200)])
        clock = Clock()
        result = retry.fetch(
            "req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 1.0
        )
        self.assertEqual(result.status, 200)
        self.assertEqual(clock.slept, [0.1, 0.2])

    def test_jitter_can_halve_the_delay(self):
        send = Transport([Response(500), Response(500), Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, base_delay_ms=100, rand=lambda: 0.0)
        self.assertEqual(clock.slept, [0.05, 0.1])

    def test_success_never_sleeps(self):
        send = Transport([Response(200)])
        clock = Clock()
        retry.fetch("req", send, clock, attempts=3, rand=lambda: 1.0)
        self.assertEqual(clock.slept, [])


if __name__ == "__main__":
    unittest.main()
