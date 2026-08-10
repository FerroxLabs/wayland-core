#!/usr/bin/env python3
"""Apply the A-3 reference solution (self-test only)."""
import os
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def apply(repo, with_test=True):
    shutil.copyfile(
        os.path.join(HERE, "store.py"),
        os.path.join(repo, "src", "contacts", "store.py"),
    )
    if with_test:
        shutil.copyfile(
            os.path.join(HERE, "test_regression_duplicates.py"),
            os.path.join(repo, "tests", "test_regression_duplicates.py"),
        )


def apply_symptom_only(repo):
    """A deliberately wrong fix: patch the web form, leave the store broken."""
    path = os.path.join(repo, "src", "contacts", "web.py")
    with open(path, "r", encoding="utf-8") as fh:
        source = fh.read()
    source = source.replace(
        '"email": form.get("email", ""),',
        '"email": form.get("email", "").strip().lower(),',
    )
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(source)


if __name__ == "__main__":
    apply(sys.argv[1])
