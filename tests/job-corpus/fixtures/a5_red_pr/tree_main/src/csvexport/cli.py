"""Command line entry point."""

import sys

from .rows import sample_rows


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    rows = sample_rows()
    print("%d rows available" % len(rows))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
