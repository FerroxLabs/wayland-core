#!/usr/bin/env python3
"""Apply the A-1 reference solution to a built fixture (self-test only)."""
import os
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def apply(repo):
    shutil.copyfile(
        os.path.join(HERE, "unitkit_convert.py"),
        os.path.join(repo, "unitkit", "convert.py"),
    )
    shutil.copyfile(
        os.path.join(HERE, "unitkit__init__.py"),
        os.path.join(repo, "unitkit", "__init__.py"),
    )


if __name__ == "__main__":
    apply(sys.argv[1])
