"""Write rows out for the customer."""

import csv
import datetime

HEADER = ["id", "customer", "amount", "created_at"]


def _format_cell(value):
    if isinstance(value, datetime.datetime):
        moment = value.astimezone(datetime.timezone.utc).replace(tzinfo=None)
        return moment.strftime("%Y-%m-%dT%H:%M:%SZ")
    return str(value)


def export_csv(rows, out):
    """Write ``rows`` to the writable text stream ``out``."""
    writer = csv.writer(out, lineterminator="\n")
    writer.writerow(HEADER)
    for row in rows:
        writer.writerow([_format_cell(getattr(row, name)) for name in HEADER])
    return out
