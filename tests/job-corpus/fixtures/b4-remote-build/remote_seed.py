#!/usr/bin/env python3
"""Prepare the BUILD HOST for row B-4. Run this on the remote machine only.

Plants the build seed that exists nowhere else, so a package carrying its
fingerprint proves the build really happened here and not on the laptop.

  python3 remote_seed.py --reset
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shutil
import sys


def home():
    return os.path.abspath(os.environ.get(
        "JOBCORPUS_B4_HOME", os.path.join(os.path.expanduser("~"), ".jobcorpus-b4")))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reset", action="store_true",
                    help="clear the ledger, lock and any partial output first")
    ap.add_argument("--workspace", default=None,
                    help="project checkout on this host, so stale partials can be cleared")
    args = ap.parse_args()

    root = home()
    os.makedirs(root, exist_ok=True)
    seed_path = os.path.join(root, "remote-only-seed.txt")

    if args.reset:
        for name in ("ledger.jsonl", "build.lock"):
            try:
                os.unlink(os.path.join(root, name))
            except OSError:
                pass
        if args.workspace:
            shutil.rmtree(os.path.join(args.workspace, "dist"), ignore_errors=True)

    if args.reset or not os.path.exists(seed_path):
        with open(seed_path, "w", encoding="utf-8") as fh:
            fh.write(secrets.token_hex(32) + "\n")
        os.chmod(seed_path, 0o600)

    with open(seed_path, "rb") as fh:
        fp = hashlib.sha256(fh.read().strip()).hexdigest()[:16]

    print(json.dumps({"home": root, "seed_fingerprint": fp, "reset": args.reset}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
