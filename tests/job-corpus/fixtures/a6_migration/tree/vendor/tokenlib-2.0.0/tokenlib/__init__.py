"""tokenlib 2.0.0 — signed, expiring tokens."""

import base64
import hashlib
import hmac
import json
import time

__version__ = "2.0.0"

DEFAULT_ALGORITHM = "sha256"
LEGACY_PREFIX = "v1"
PREFIX = "v2"


class TokenError(Exception):
    """Base class for every token failure."""


class TokenInvalid(TokenError):
    """The token is malformed, or its signature does not check out."""


class TokenExpired(TokenError):
    """The token was well formed but has expired."""


def _encode(payload, exp):
    raw = json.dumps({"p": payload, "exp": int(exp)}, sort_keys=True).encode("utf-8")
    return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")


def _decode(body):
    padded = body + "=" * (-len(body) % 4)
    return json.loads(base64.urlsafe_b64decode(padded.encode("ascii")).decode("utf-8"))


def _sign(body, secret, algorithm):
    digest = getattr(hashlib, algorithm)
    return hmac.new(secret.encode("utf-8"), body.encode("ascii"), digest).hexdigest()


def issue_token(payload, ttl_seconds, secret=None, algorithm=DEFAULT_ALGORITHM):
    """Mint a token that expires ``ttl_seconds`` from now."""
    body = _encode(payload, time.time() + int(ttl_seconds))
    return "%s.%s.%s" % (PREFIX, body, _sign(body, secret, algorithm))


def verify(token, secret=None, algorithm=DEFAULT_ALGORITHM, legacy_algorithms=()):
    """Return the payload, or raise :class:`TokenError`."""
    parts = str(token).split(".")
    if len(parts) != 3:
        raise TokenInvalid("malformed token")
    version, body, signature = parts
    if version == PREFIX:
        candidates = [algorithm]
    elif version == LEGACY_PREFIX:
        candidates = list(legacy_algorithms)
        if not candidates:
            raise TokenInvalid(
                "token was issued by tokenlib 1.x; pass legacy_algorithms to accept it"
            )
    else:
        raise TokenInvalid("unknown token version %r" % version)

    for candidate in candidates:
        if hmac.compare_digest(signature, _sign(body, secret, candidate)):
            break
    else:
        raise TokenInvalid("signature does not check out")

    try:
        data = _decode(body)
    except (ValueError, UnicodeDecodeError) as exc:
        raise TokenInvalid("payload is not readable") from exc
    if data["exp"] < time.time():
        raise TokenExpired("token expired at %s" % data["exp"])
    return data["p"]
