"""Drop dead tokens from the session table."""

import tokenlib

from ..config import load_token_config


def purge_expired(tokens, config=None):
    """Return only the tokens that still verify."""
    cfg = config or load_token_config()
    keep = []
    for token in tokens:
        if tokenlib.verify(token, secret=cfg["secret"]) is not None:
            keep.append(token)
    return keep
