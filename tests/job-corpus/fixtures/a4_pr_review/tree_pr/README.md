# gatekeeper

Rate limiting for the public API edge. One process, many request threads.

```python
limiter = RateLimiter(limit=100, window=60)
if not limiter.allow(api_key):
    return 429
```

The limiter uses a sliding window: the last `window` seconds of requests are
kept per key, so a caller cannot burst across a window boundary.

Call `sweep()` periodically to forget idle keys.

Run the tests with:

```
python3 -m unittest discover -s tests -t .
```
