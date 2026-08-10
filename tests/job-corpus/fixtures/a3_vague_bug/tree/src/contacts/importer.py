"""Nightly import from the old system's CSV export."""


def import_csv(store, text):
    """Import ``name,email`` rows. Returns the ids that were written.

    The old system was not fussy about how addresses were typed, so we tidy
    them up on the way in.
    """
    written = []
    for row in text.splitlines():
        if not row.strip():
            continue
        name, _, email = row.partition(",")
        written.append(
            store.save({"name": name.strip(), "email": email.strip().lower()})
        )
    return written
