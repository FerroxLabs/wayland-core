#!/usr/bin/env python3
"""Build the A-4 fixture repository: main plus the PR branch."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, git, here  # noqa: E402


def main():
    dest = dest_from_argv("a4_pr_review")
    root = here(__file__)
    repo = fresh_repo(dest)
    copy_tree(os.path.join(root, "tree_main"), repo)
    commit_all(repo, "gatekeeper: fixed-window rate limiter", tag="baseline")
    git(repo, "checkout", "-q", "-b", "pr/sliding-window")
    copy_tree(os.path.join(root, "tree_pr"), repo)
    commit_all(repo, "limiter: sliding window instead of fixed window")
    print(repo)


if __name__ == "__main__":
    main()
