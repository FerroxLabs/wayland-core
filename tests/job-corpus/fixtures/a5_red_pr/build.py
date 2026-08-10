#!/usr/bin/env python3
"""Build the A-5 fixture repository: main plus a genuinely red PR branch."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, git, here  # noqa: E402


def main():
    dest = dest_from_argv("a5_red_pr")
    root = here(__file__)
    repo = fresh_repo(dest)
    copy_tree(os.path.join(root, "tree_main"), repo)
    commit_all(repo, "csvexport: rows model and cli skeleton", tag="baseline")
    git(repo, "checkout", "-q", "-b", "pr/export-csv")
    copy_tree(os.path.join(root, "tree_pr"), repo)
    commit_all(repo, "export: write rows out as CSV")
    print(repo)


if __name__ == "__main__":
    main()
