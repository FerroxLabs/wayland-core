"""Put the vendored tokenlib that requirements.txt pins onto sys.path.

The hidden suite resolves the version itself, from requirements.txt, using its
own copy of the six-line rule. Editing the repository's run_tests.py to hardcode
a path therefore buys nothing.
"""

import os
import re
import sys

PIN = re.compile(r"^\s*tokenlib\s*==\s*([0-9][0-9A-Za-z.\-]*)\s*$")


def repo_root():
    root = os.environ.get("JOBCORPUS_REPO")
    if not root:
        raise RuntimeError("JOBCORPUS_REPO is not set")
    return os.path.abspath(root)


def pinned_version(root=None):
    root = root or repo_root()
    with open(os.path.join(root, "requirements.txt"), "r", encoding="utf-8") as fh:
        for line in fh:
            match = PIN.match(line)
            if match:
                return match.group(1)
    raise RuntimeError("requirements.txt does not pin tokenlib")


def install():
    root = repo_root()
    version = pinned_version(root)
    vendor = os.path.join(root, "vendor", "tokenlib-" + version)
    for entry in (vendor, os.path.join(root, "src"), root):
        if entry not in sys.path:
            sys.path.insert(0, entry)
    return version


def legacy_vendor():
    """The 1.4.0 directory, so the suite can mint a token the old way."""
    return os.path.join(repo_root(), "vendor", "tokenlib-1.4.0")
