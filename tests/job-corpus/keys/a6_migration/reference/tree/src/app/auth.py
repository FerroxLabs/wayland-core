"""Issuing and checking session tokens."""

import tokenlib

from .config import load_token_config


def issue(payload, config=None):
    """Mint a token for ``payload``."""
    cfg = config or load_token_config()
    return tokenlib.issue_token(
        payload,
        cfg["ttl_seconds"],
        secret=cfg["secret"],
        algorithm=cfg["algorithm"],
    )


def check(token, config=None):
    """Return the payload carried by ``token``, or ``None`` if it is no good.

    tokenlib 2.0.0 raises instead of returning ``None``; callers here still use
    the ``None`` convention, so the exception is absorbed at this boundary.
    Sessions minted by tokenlib 1.x are still accepted while
    ``legacy_algorithms`` is configured.
    """
    cfg = config or load_token_config()
    try:
        return tokenlib.verify(
            token,
            secret=cfg["secret"],
            algorithm=cfg["algorithm"],
            legacy_algorithms=cfg["legacy_algorithms"],
        )
    except tokenlib.TokenError:
        return None
