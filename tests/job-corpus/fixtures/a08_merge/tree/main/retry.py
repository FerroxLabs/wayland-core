"""Retry an HTTP request that failed for a reason worth retrying."""

import random


class RetryError(Exception):
    """Raised when the request could not be completed."""


def is_retryable(response):
    """429 and 5xx are worth another go; nothing else is."""
    return response.status == 429 or response.status >= 500


def fetch(request, send, sleep, attempts=3, base_delay_ms=100, rand=None):
    """Send `request`, retrying while the response is retryable.

    `send(request)` returns a response with `.status` and `.headers`.
    `sleep(seconds)` is called between attempts.
    `rand()` returns a float in [0, 1) and drives the backoff jitter. Inject it
    to make the delays deterministic in tests.
    """
    if attempts < 1:
        raise ValueError("attempts must be at least 1")
    if rand is None:
        rand = random.random
    for attempt in range(attempts):
        response = send(request)
        if not is_retryable(response):
            return response
        if attempt == attempts - 1:
            break
        delay_ms = base_delay_ms * (2 ** attempt)
        delay_ms = int(delay_ms * (0.5 + 0.5 * rand()))
        sleep(delay_ms / 1000.0)
    raise RetryError("gave up after %d attempts" % attempts)
