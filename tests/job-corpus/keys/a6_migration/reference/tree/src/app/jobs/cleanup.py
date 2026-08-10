"""Drop dead tokens from the session table."""

import tokenlib

from ..config import load_token_config


def purge_expired(tokens, config=None):
    """Return only the tokens that still verify."""
    cfg = config or load_token_config()
    keep = []
    for token in tokens:
        try:
            tokenlib.verify(
                token,
                secret=cfg["secret"],
                algorithm=cfg["algorithm"],
                legacy_algorithms=cfg["legacy_algorithms"],
            )
        except tokenlib.TokenError:
            continue
        keep.append(token)
    return keep
