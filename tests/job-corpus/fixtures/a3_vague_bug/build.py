#!/usr/bin/env python3
"""Build the A-3 fixture repository."""
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from _build_lib import commit_all, copy_tree, dest_from_argv, fresh_repo, git, here  # noqa: E402


def main():
    dest = dest_from_argv("a3_vague_bug")
    root = here(__file__)
    repo = fresh_repo(dest)
    copy_tree(os.path.join(root, "tree"), repo)
    shutil.copyfile(
        os.path.join(root, "TICKET-8871.md"), os.path.join(repo, "TICKET-8871.md")
    )
    commit_all(repo, "contacts: address book with web form and importer", tag="baseline")
    git(repo, "tag", "pre-fix")
    print(repo)


if __name__ == "__main__":
    main()
