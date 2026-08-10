There's a PR up on this repo: branch `pr/sliding-window`, against `main`. It
swaps our fixed-window rate limiter for a sliding-window one. It's going in
front of the public API edge so I'd like a proper look at it before it lands.

Review it. Please leave your findings in `review.json` at the repo root, as a
JSON list, one object per finding:

```json
[
  {"file": "src/gatekeeper/limiter.py", "line": 42,
   "severity": "blocker", "message": "..."}
]
```

`severity` is one of `blocker`, `major`, `minor`, `nit`, `question`. Use
`blocker` or `major` for anything you think must be fixed before this merges,
and the lighter ones for anything you'd merely mention.
