My PR is red and I've got three comments on it I haven't got round to. Branch
is `pr/export-csv`. Could you sort it out on that branch, please — I don't want
to have to re-open a new one.

CI runs the two checks in `.ci/checks.json`. Both need to be green.

The review comments:

> **jo** on `src/csvexport/exporter.py`
> The timestamp column is being rendered as `05/03/2024 14:00` in whatever zone
> the box happens to be in. Two of our customers are in Asia and they're seeing
> the wrong day in their exports. It needs to be ISO-8601 in UTC —
> `2024-03-05T14:00:00Z` — and rows that arrive with an offset need converting,
> not truncating.

> **jo** on `src/csvexport/exporter.py`
> Can `export_csv` take a writable text stream rather than a path? Signature
> `export_csv(rows, out)`. We want to stream this straight into an HTTP response
> without going via a temp file. Please update the CLI caller too.

> **sam** on `README.md`
> README still says the export is tab-separated. It isn't, and someone is going
> to write an importer off the back of that.

Don't touch the lint rules or the check definitions to get green — if a check
is failing, fix the code.
