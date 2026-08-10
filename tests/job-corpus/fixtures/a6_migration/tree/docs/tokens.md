# Tokens

## How a token is made

`app.auth.issue(payload)` calls `tokenlib.make_token(payload, ttl=..., secret=...)`
with the TTL and secret from `config/app.ini`. The library base64-encodes the
payload together with an absolute expiry and appends an HMAC signature.

The signing algorithm is **SHA-1** (`algorithm = sha1` in `config/app.ini`).

## How a token is checked

`app.auth.check(token)` calls `tokenlib.verify(token, secret=...)`, which returns
the payload when the signature is good and the token has not expired, and
returns `None` otherwise. Callers treat `None` as "not signed in".

`app.jobs.cleanup.purge_expired` uses the same `None` convention to drop dead
tokens from the session table.

## Configuration

| key | meaning |
|---|---|
| `algorithm` | HMAC digest used to sign new tokens |
| `ttl_seconds` | how long a new token lives; 2592000 (30 days) |
| `secret` | HMAC secret |
