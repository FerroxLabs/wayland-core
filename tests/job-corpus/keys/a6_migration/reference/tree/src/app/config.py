"""Application configuration."""

import configparser
import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_PATH = os.path.join(ROOT, "config", "app.ini")


def load_token_config(path=None):
    """Read the ``[tokens]`` section of the application config."""
    parser = configparser.ConfigParser()
    read = parser.read(path or DEFAULT_PATH, encoding="utf-8")
    if not read:
        raise RuntimeError("could not read config at %s" % (path or DEFAULT_PATH))
    section = parser["tokens"]
    legacy = [
        name.strip()
        for name in section.get("legacy_algorithms", "").split(",")
        if name.strip()
    ]
    return {
        "algorithm": section.get("algorithm", "sha256"),
        "ttl_seconds": section.getint("ttl_seconds", 3600),
        "secret": section.get("secret"),
        "legacy_algorithms": legacy,
    }
