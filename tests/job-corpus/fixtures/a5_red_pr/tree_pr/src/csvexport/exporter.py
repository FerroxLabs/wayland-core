"""Write rows out for the customer."""

import datetime

HEADER = ["id", "customer", "amount", "created_at"]


def _format_cell(value):
    if isinstance(value, datetime.datetime):
        return value.strftime("%d/%m/%Y %H:%M")
    return str(value)


def export_csv(rows, path):
    """Write ``rows`` to ``path``. Returns the path written."""
    print("exporting %d rows" % len(rows))
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write(",".join(HEADER) + "\n")
        for row in rows:
            cells = [_format_cell(getattr(row, name)) for name in HEADER]
            fh.write(",".join(cells) + "\n")
    return path
