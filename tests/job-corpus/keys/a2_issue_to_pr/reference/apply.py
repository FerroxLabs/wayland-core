#!/usr/bin/env python3
"""Apply the A-2 reference solution (self-test only)."""
import os
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def apply(repo):
    shutil.copyfile(
        os.path.join(HERE, "parser.py"),
        os.path.join(repo, "src", "receipts", "parser.py"),
    )


if __name__ == "__main__":
    apply(sys.argv[1])
