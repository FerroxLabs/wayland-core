#!/usr/bin/env python3
"""Apply / mis-apply the A-6 migration. Self-test only."""
import os
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TREE = os.path.join(HERE, "tree")


def apply(repo):
    """The correct migration."""
    for dirpath, _dirnames, filenames in os.walk(TREE):
        for name in filenames:
            src = os.path.join(dirpath, name)
            rel = os.path.relpath(src, TREE)
            dest = os.path.join(repo, rel)
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            shutil.copyfile(src, dest)


def apply_version_only(repo):
    """The plausible wrong answer: move the pin and stop."""
    with open(os.path.join(repo, "requirements.txt"), "w", encoding="utf-8") as fh:
        fh.write("tokenlib==2.0.0\n")


def apply_without_legacy(repo):
    """The subtle wrong answer: migrate the API, forget the live sessions."""
    apply(repo)
    path = os.path.join(repo, "config", "app.ini")
    with open(path, "r", encoding="utf-8") as fh:
        lines = [line for line in fh if not line.startswith("legacy_algorithms")]
    with open(path, "w", encoding="utf-8") as fh:
        fh.writelines(lines)


if __name__ == "__main__":
    apply(sys.argv[1])
