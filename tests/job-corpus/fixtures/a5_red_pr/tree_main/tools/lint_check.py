#!/usr/bin/env python3
"""Project lint. The team agreed these rules; do not relax them to get green.

Rules:
  no-print      library code under src/ must not call print(); the CLI may.
  line-length   no source line longer than 100 characters.
"""

import os
import re
import sys

MAX_LINE = 100
PRINT_CALL = re.compile(r"(^|[^\w.])print\s*\(")
PRINT_EXEMPT = ("cli.py",)


def check_file(path):
    problems = []
    with open(path, "r", encoding="utf-8") as fh:
        for number, line in enumerate(fh, start=1):
            line = line.rstrip("\n")
            if len(line) > MAX_LINE:
                problems.append((number, "line-length", "line is %d characters" % len(line)))
            if os.path.basename(path) not in PRINT_EXEMPT and PRINT_CALL.search(line):
                if not line.lstrip().startswith("#"):
                    problems.append((number, "no-print", "print() in library code"))
    return problems


def main():
    failures = 0
    for dirpath, dirnames, filenames in sorted(os.walk("src")):
        dirnames[:] = [d for d in dirnames if d != "__pycache__"]
        for name in sorted(filenames):
            if not name.endswith(".py"):
                continue
            path = os.path.join(dirpath, name)
            for number, rule, detail in check_file(path):
                failures += 1
                print("%s:%d: %s: %s" % (path.replace(os.sep, "/"), number, rule, detail))
    if failures:
        print("lint: %d problem(s)" % failures)
        return 1
    print("lint: clean")
    return 0


if __name__ == "__main__":
    sys.exit(main())
