"""Retry an HTTP request that failed for a reason worth retrying."""


class RetryError(Exception):
    """Raised when the request could not be completed."""


def is_retryable(response):
    """429 and 5xx are worth another go; nothing else is."""
    return response.status == 429 or response.status >= 500


def fetch(request, send, sleep, attempts=3, base_delay_ms=100, budget_ms=None):
    """Send `request`, retrying while the response is retryable.

    `send(request)` returns a response with `.status` and `.headers`.
    `sleep(seconds)` is called between attempts.
    `budget_ms` caps the total time spent waiting between attempts.
    """
    if attempts < 1:
        raise ValueError("attempts must be at least 1")
    waited_ms = 0
    for attempt in range(attempts):
        response = send(request)
        if not is_retryable(response):
            return response
        if attempt == attempts - 1:
            break
        retry_after = response.headers.get("Retry-After")
        if retry_after is not None:
            delay_ms = int(float(retry_after) * 1000)
        else:
            delay_ms = base_delay_ms
        if budget_ms is not None and waited_ms + delay_ms > budget_ms:
            raise RetryError("retry budget of %dms exhausted" % budget_ms)
        waited_ms = waited_ms + delay_ms
        sleep(delay_ms / 1000.0)
    raise RetryError("gave up after %d attempts" % attempts)
