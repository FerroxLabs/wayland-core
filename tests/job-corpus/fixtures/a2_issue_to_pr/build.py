#!/usr/bin/env python3
"""Build the A-2 fixture repository."""
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, here  # noqa: E402


def main():
    dest = dest_from_argv("a2_issue_to_pr")
    root = here(__file__)
    repo = fresh_repo(dest)
    copy_tree(os.path.join(root, "tree"), repo)
    shutil.copyfile(
        os.path.join(root, "ISSUE-412.md"), os.path.join(repo, "ISSUE-412.md")
    )
    commit_all(repo, "receipts: parse expense lines", tag="baseline")
    print(repo)


if __name__ == "__main__":
    main()
