"""Command line entry point for receipts."""

import sys

from .parser import parse_expenses


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv:
        with open(argv[0], "r", encoding="utf-8") as fh:
            text = fh.read()
    else:
        text = sys.stdin.read()

    result = parse_expenses(text)
    for line_number, raw, reason in result.errors:
        print("line %d: could not read %r (%s)" % (line_number, raw, reason))
    print("total: %.2f" % result.total)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
