#!/usr/bin/env python3
"""Build a clean B-2 workspace (the monthly-billing repo).

  python3 seed_workspace.py --dest /path/to/ws
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def git(ws, *args):
    subprocess.run(["git"] + list(args), cwd=ws, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dest", required=True)
    args = ap.parse_args()
    dest = os.path.abspath(args.dest)
    if os.path.exists(dest):
        shutil.rmtree(dest)
    shutil.copytree(os.path.join(HERE, "seed"), dest)
    git(dest, "init", "-q", "-b", "main")
    git(dest, "config", "user.email", "billing@fixture.local")
    git(dest, "config", "user.name", "Billing Fixture")
    git(dest, "add", ".")
    git(dest, "commit", "-q", "-m", "seed: monthly billing inputs")
    print(dest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
