#!/usr/bin/env python3
"""Build the A-6 fixture repository."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, here  # noqa: E402


def main():
    dest = dest_from_argv("a6_migration")
    root = here(__file__)
    repo = fresh_repo(dest)
    copy_tree(os.path.join(root, "tree"), repo)
    commit_all(repo, "tokenapp: signed sessions on tokenlib 1.4.0", tag="baseline")
    print(repo)


if __name__ == "__main__":
    main()
