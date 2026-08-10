"""Issuing and checking session tokens."""

import tokenlib

from .config import load_token_config


def issue(payload, config=None):
    """Mint a token for ``payload``."""
    cfg = config or load_token_config()
    return tokenlib.make_token(
        payload,
        ttl=cfg["ttl_seconds"],
        secret=cfg["secret"],
        algorithm=cfg["algorithm"],
    )


def check(token, config=None):
    """Return the payload carried by ``token``, or ``None`` if it is no good."""
    cfg = config or load_token_config()
    return tokenlib.verify(token, secret=cfg["secret"])
