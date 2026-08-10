"""Command line entry point."""

import sys

from .exporter import export_csv
from .rows import sample_rows


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    rows = sample_rows()
    if not argv:
        print("usage: cli <output-path>")
        return 2
    export_csv(rows, argv[0])
    print("wrote %s" % argv[0])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
