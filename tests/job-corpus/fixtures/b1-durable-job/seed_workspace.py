#!/usr/bin/env python3
"""Build a clean B-1 workspace: a git repo containing the warehouse handoff.

  python3 seed_workspace.py --dest /path/to/ws --endpoint http://127.0.0.1:8731
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
    ap.add_argument("--endpoint", required=True)
    args = ap.parse_args()

    dest = os.path.abspath(args.dest)
    if os.path.exists(dest):
        shutil.rmtree(dest)
    shutil.copytree(os.path.join(HERE, "seed"), dest)
    with open(os.path.join(dest, "shipping-endpoint.txt"), "w", encoding="utf-8") as fh:
        fh.write(args.endpoint.rstrip("/") + "\n")

    git(dest, "init", "-q", "-b", "main")
    git(dest, "config", "user.email", "warehouse@fixture.local")
    git(dest, "config", "user.name", "Warehouse Fixture")
    git(dest, "add", ".")
    git(dest, "commit", "-q", "-m", "seed: tonight's orders")
    print(dest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
