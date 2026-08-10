#!/usr/bin/env python3
"""Run the suite against the tokenlib version pinned in requirements.txt."""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
PIN = re.compile(r"^\s*tokenlib\s*==\s*([0-9][0-9A-Za-z.\-]*)\s*$")


def vendor_path():
    with open(os.path.join(ROOT, "requirements.txt"), "r", encoding="utf-8") as fh:
        for line in fh:
            match = PIN.match(line)
            if match:
                path = os.path.join(ROOT, "vendor", "tokenlib-" + match.group(1))
                if not os.path.isdir(path):
                    raise SystemExit("no vendored tokenlib at %s" % path)
                return path
    raise SystemExit("requirements.txt does not pin tokenlib")


def main():
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [vendor_path(), os.path.join(ROOT, "src"), ROOT]
    )
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.call(
        [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", "."],
        cwd=ROOT,
        env=env,
    )


if __name__ == "__main__":
    sys.exit(main())
