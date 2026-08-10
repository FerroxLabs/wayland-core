#!/usr/bin/env python3
"""Build the A-1 fixture repository."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, here  # noqa: E402


def main():
    dest = dest_from_argv("a1_cold_start")
    src = os.path.join(here(__file__), "tree")
    repo = fresh_repo(dest)
    copy_tree(src, repo)
    commit_all(repo, "unitkit: fahrenheit to celsius", tag="baseline")
    print(repo)


if __name__ == "__main__":
    main()
