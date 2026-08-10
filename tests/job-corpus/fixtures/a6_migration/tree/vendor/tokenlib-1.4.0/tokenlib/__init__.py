"""tokenlib 1.4.0 — signed, expiring tokens."""

import base64
import hashlib
import hmac
import json
import time

__version__ = "1.4.0"

DEFAULT_ALGORITHM = "sha1"


def _encode(payload, exp):
    raw = json.dumps({"p": payload, "exp": int(exp)}, sort_keys=True).encode("utf-8")
    return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")


def _decode(body):
    padded = body + "=" * (-len(body) % 4)
    return json.loads(base64.urlsafe_b64decode(padded.encode("ascii")).decode("utf-8"))


def _sign(body, secret, algorithm):
    digest = getattr(hashlib, algorithm)
    return hmac.new(secret.encode("utf-8"), body.encode("ascii"), digest).hexdigest()


def make_token(payload, ttl=3600, secret=None, algorithm=DEFAULT_ALGORITHM):
    """Mint a token that expires ``ttl`` seconds from now."""
    body = _encode(payload, time.time() + int(ttl))
    return "v1.%s.%s" % (body, _sign(body, secret, algorithm))


def verify(token, secret=None):
    """Return the payload, or ``None`` when the token is no good."""
    parts = str(token).split(".")
    if len(parts) != 3:
        return None
    version, body, signature = parts
    if version != "v1":
        return None
    if not hmac.compare_digest(signature, _sign(body, secret, DEFAULT_ALGORITHM)):
        return None
    try:
        data = _decode(body)
    except (ValueError, UnicodeDecodeError):
        return None
    if data["exp"] < time.time():
        return None
    return data["p"]
