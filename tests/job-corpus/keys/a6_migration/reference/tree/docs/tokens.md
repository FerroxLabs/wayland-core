# Tokens

## How a token is made

`app.auth.issue(payload)` calls
`tokenlib.issue_token(payload, ttl_seconds, secret=...)` with the TTL and secret
from `config/app.ini`. The library base64-encodes the payload together with an
absolute expiry and appends an HMAC signature.

The signing algorithm is **SHA-256** (`algorithm = sha256` in `config/app.ini`).

## How a token is checked

`app.auth.check(token)` calls `tokenlib.verify(...)`. Since tokenlib 2.0.0 that
call **raises** — `TokenExpired` for a token that has run out, `TokenInvalid`
for anything else — so `app.auth.check` absorbs `TokenError` and keeps returning
`None` for its own callers, who treat `None` as "not signed in".

`app.jobs.cleanup.purge_expired` catches the same exceptions to drop dead tokens
from the session table.

## Sessions from the previous release

Tokens minted before the 2.0.0 upgrade carry a `v1.` prefix and a legacy SHA-1
signature. tokenlib 2.0.0 rejects those unless it is told to accept them, so
`legacy_algorithms = sha1` is set in `config/app.ini` and passed to `verify()`.
Sessions last 30 days, so that line can be removed 30 days after the upgrade
was deployed; until then, removing it signs every existing user out.

## Configuration

| key | meaning |
|---|---|
| `algorithm` | HMAC digest used to sign new tokens (sha256) |
| `ttl_seconds` | how long a new token lives; 2592000 (30 days) |
| `secret` | HMAC secret |
| `legacy_algorithms` | digests accepted on `v1.` tokens from the previous release |
