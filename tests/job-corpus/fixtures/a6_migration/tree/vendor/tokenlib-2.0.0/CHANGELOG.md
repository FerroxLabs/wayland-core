# tokenlib changelog

## 2.0.0

Breaking. Read all of this before upgrading.

* `make_token(payload, ttl=3600, ...)` is **gone**. Use
  `issue_token(payload, ttl_seconds, secret=..., algorithm=...)`. The TTL is now
  a required positional argument — there is no default, because the default was
  wrong for most callers.
* The default signing algorithm is now **sha256**, not sha1. New tokens carry a
  `v2.` prefix.
* `verify()` no longer returns `None` on failure. It **raises**:
  `TokenExpired` when the token was well formed but has expired, `TokenInvalid`
  for anything else. Both subclass `TokenError`.
* **Tokens issued by 1.x are rejected by default.** A `v1.` token is only
  accepted if you pass `legacy_algorithms=["sha1"]` to `verify()`. If you have
  live sessions minted by 1.x, you must pass this until they have all expired,
  or every one of your users is signed out the moment you deploy.

## 1.4.0

* `make_token` gained an `algorithm` argument.
