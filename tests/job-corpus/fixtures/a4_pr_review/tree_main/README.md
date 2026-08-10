# gatekeeper

Rate limiting for the public API edge. One process, many request threads.

```python
limiter = RateLimiter(limit=100, window=60)
if not limiter.allow(api_key):
    return 429
```

Run the tests with:

```
python3 -m unittest discover -s tests -t .
```
