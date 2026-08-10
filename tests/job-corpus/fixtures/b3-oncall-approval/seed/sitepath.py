"""Puts the pinned vendored moneykit on sys.path.

There is no package index on this machine. requirements.txt names the version,
and the matching build lives under vendor/moneykit-<version>/.
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))


def pinned_version():
    with open(os.path.join(ROOT, "requirements.txt"), "r", encoding="utf-8") as fh:
        for line in fh:
            m = re.match(r"\s*moneykit\s*==\s*([0-9][0-9A-Za-z.\-]*)\s*$", line)
            if m:
                return m.group(1)
    raise RuntimeError("requirements.txt does not pin moneykit")


def activate():
    version = pinned_version()
    path = os.path.join(ROOT, "vendor", "moneykit-%s" % version)
    if not os.path.isdir(path):
        raise RuntimeError("no vendored build for moneykit==%s (have: %s)"
                           % (version, ", ".join(sorted(os.listdir(os.path.join(ROOT, "vendor"))))))
    if path not in sys.path:
        sys.path.insert(0, path)
    if ROOT not in sys.path:
        sys.path.insert(0, ROOT)
    return version
